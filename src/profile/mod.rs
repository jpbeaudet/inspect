//! Server profiles produced by auto-discovery.
//!
//! A profile is the durable, on-disk model of what `inspect` learned about a
//! namespace's host: containers, host services, listening ports, available
//! remote tooling, plus user-curated sections (`groups`, `aliases`,
//! `local_overrides`) that survive re-discovery untouched.

pub mod cache;
pub mod runtime;
pub mod schema;

#[allow(unused_imports)]
pub use cache::{is_stale, load_profile, profile_path, save_profile, DEFAULT_TTL_DAYS};
#[allow(unused_imports)]
pub use runtime::{
    clear_all as clear_runtime_all, clear_runtime, inventory_age, is_runtime_stale, load_runtime,
    runtime_path, runtime_ttl, save_runtime, RuntimeSnapshot, ServiceRuntime, SourceInfo,
    SourceMode, RUNTIME_SCHEMA_VERSION,
};
#[allow(unused_imports)]
pub use schema::{
    HealthStatus, Image, LogDriver, Mount, Network, Port, Profile, RemoteTooling, Service, Volume,
    PROFILE_SCHEMA_VERSION,
};
