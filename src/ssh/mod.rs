//! Persistent SSH session management (Phase 1).
//!
//! `inspect` shells out to the real OpenSSH client and uses `ControlMaster`
//! with `ControlPersist` to keep a multiplexer running across CLI
//! invocations. The bible mandates this so that a passphrase is entered at
//! most once per terminal session.
//!
//! All sockets live in `~/.inspect/sockets/<ns>.sock` with mode 0600.

pub mod askpass;
pub mod concurrency;
pub mod exec;
pub mod master;
pub mod options;
pub mod ttl;

#[allow(unused_imports)]
pub use exec::{run_remote, RemoteOutput, RunOpts};
#[allow(unused_imports)]
pub use master::{
    check_socket, ensure_sockets_dir, exit_master, list_sockets, socket_path, start_master,
    AuthMode, ConnectOutcome, MasterStatus,
};
pub use options::SshTarget;
#[allow(unused_imports)]
pub use ttl::{default_ttl, parse_ttl, TtlSource};
