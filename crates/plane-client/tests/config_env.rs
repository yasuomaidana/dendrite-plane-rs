//! `Config::from_env` reads the process environment, so it gets a test binary of its own.

use plane_client::{Config, Error};

#[test]
fn from_env_defaults_and_overrides() {
    // SAFETY: this binary holds a single test, so nothing else touches the environment.
    unsafe {
        std::env::remove_var("PLANE_API_KEY");
        std::env::remove_var("PLANE_BASE_URL");
        std::env::remove_var("PLANE_API_PORT");
        std::env::remove_var("PLANE_WORKSPACE_SLUG");
        std::env::remove_var("PLANE_PROJECT_ID");
    }
    assert!(matches!(Config::from_env(), Err(Error::MissingToken)));

    unsafe {
        std::env::set_var("PLANE_API_KEY", "k");
        std::env::set_var("PLANE_API_PORT", "9000");
    }
    let cfg = Config::from_env().unwrap();
    assert_eq!(cfg.base_url, "http://localhost:9000");
    assert_eq!(cfg.workspace, "dendrite-ws");

    unsafe {
        std::env::set_var("PLANE_BASE_URL", "http://127.0.0.1:1");
        std::env::set_var("PLANE_WORKSPACE_SLUG", "ws");
        std::env::set_var("PLANE_PROJECT_ID", "proj");
    }
    let cfg = Config::from_env().unwrap();
    assert_eq!(
        (
            cfg.base_url.as_str(),
            cfg.workspace.as_str(),
            cfg.project_id.as_str()
        ),
        ("http://127.0.0.1:1", "ws", "proj")
    );
}
