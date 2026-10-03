//! The `Http` port, built on first use (§13.5): a command that sends nothing, `statusline`
//! above all, never pays for constructing the TLS client.

use std::sync::{Arc, OnceLock};

use tagteam_provider::http::{Http, HttpError, HttpRequest, HttpResponse};

/// Builds the real adapter on the first `send`, never before (§13.5). Concurrent first sends
/// build it once; every send after that goes to the same adapter.
pub struct LazyHttp {
    built: OnceLock<Arc<dyn Http>>,
    factory: Box<dyn Fn() -> Arc<dyn Http> + Send + Sync>,
}

impl LazyHttp {
    pub fn new(factory: impl Fn() -> Arc<dyn Http> + Send + Sync + 'static) -> Self {
        Self {
            built: OnceLock::new(),
            factory: Box::new(factory),
        }
    }

    /// Whether a send has built the adapter yet.
    pub fn is_built(&self) -> bool {
        self.built.get().is_some()
    }
}

impl Http for LazyHttp {
    fn send(&self, req: &HttpRequest) -> Result<HttpResponse, HttpError> {
        self.built.get_or_init(|| (self.factory)()).send(req)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use serde_json::json;
    use tagteam_cc::ClaudeCode;
    use tagteam_cc::live::{LiveStore, Platform};
    use tagteam_core::{CLAUDE_CODE, ProviderId};
    use tagteam_provider::http::{Method, NoHttp, ScriptedHttp};
    use tagteam_provider::profile::RunShell;
    use tagteam_provider::{Clock, Env, FakeClock, FakeKeychain};

    use super::*;
    use crate::engine::{Engine, EngineConfig};
    use crate::oracle::{CachingOracle, HttpOracle};
    use crate::registry::ProviderRegistry;
    use crate::settings::Settings;
    use crate::vault::{KeychainVault, Vault};
    use crate::views::StatusView;

    const URL: &str = "https://usage.invalid/ping";

    /// A lazy client whose factory hands out `inner`, and how many times it has run.
    fn counting(inner: Arc<dyn Http>) -> (Arc<LazyHttp>, Arc<AtomicUsize>) {
        let builds = Arc::new(AtomicUsize::new(0));
        let counter = builds.clone();
        let lazy = LazyHttp::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            inner.clone()
        });
        (Arc::new(lazy), builds)
    }

    fn ping() -> HttpRequest {
        HttpRequest::get(URL, Duration::from_secs(5))
    }

    #[test]
    fn the_adapter_is_built_on_the_first_send_and_only_then() {
        let scripted = Arc::new(ScriptedHttp::new());
        scripted.push_json(Method::Get, URL, 200, json!({"ok": true}));
        let (lazy, builds) = counting(scripted.clone());
        assert!(!lazy.is_built());
        assert_eq!(builds.load(Ordering::SeqCst), 0);

        assert_eq!(lazy.send(&ping()).unwrap().status, 200);
        assert!(lazy.is_built());
        assert_eq!(lazy.send(&ping()).unwrap().status, 200);
        assert_eq!(builds.load(Ordering::SeqCst), 1, "built once, then reused");
        assert_eq!(
            scripted.count(Method::Get, URL),
            2,
            "every send reaches the adapter"
        );
    }

    #[test]
    fn concurrent_first_sends_build_it_once() {
        let builds = Arc::new(AtomicUsize::new(0));
        let counter = builds.clone();
        let lazy = LazyHttp::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            // Widens the window in which a second builder could start.
            std::thread::sleep(Duration::from_millis(20));
            Arc::new(NoHttp) as Arc<dyn Http>
        });
        let barrier = Barrier::new(8);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    barrier.wait();
                    assert!(lazy.send(&ping()).is_err(), "NoHttp refuses every send");
                });
            }
        });
        assert_eq!(builds.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_engine_over_a_lazy_client_does_not_build_it() {
        // §13.5: building the engine, its oracle included, and reading a view send nothing,
        // so the adapter is never constructed.
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path());
        let kc = Arc::new(FakeKeychain::new());
        let (lazy, builds) = counting(Arc::new(NoHttp));
        let http: Arc<dyn Http> = lazy.clone();
        let clock: Arc<dyn Clock> = Arc::new(FakeClock::new(1_790_000_000_000));
        let engine = Engine::new(EngineConfig {
            env,
            registry: ProviderRegistry::new().with(Arc::new(ClaudeCode::with_store(
                LiveStore::new(kc.clone(), Platform::MacOs),
            ))),
            vault: Vault::new(Box::new(KeychainVault::new(kc))),
            oracle: Arc::new(CachingOracle::new(HttpOracle::new(
                http.clone(),
                clock.clone(),
            ))),
            clock,
            http,
            default_provider: ProviderId::new(CLAUDE_CODE),
            settings: Settings::default(),
            process: Arc::new(tagteam_provider::liveness::FakeProcessProbe::new()),
            spawner: Arc::new(tagteam_provider::process::ScriptedSpawner::new()),
            run_shell: RunShell::Outside,
        });
        assert!(matches!(
            engine.status(&ProviderId::new(CLAUDE_CODE)).unwrap(),
            StatusView::NoLogin
        ));
        assert!(!lazy.is_built());
        assert_eq!(builds.load(Ordering::SeqCst), 0);
    }
}
