#[cfg_attr(target_os = "linux", path = "server.rs")]
#[cfg_attr(not(target_os = "linux"), path = "non_linux_server.rs")]
pub mod server;

#[cfg_attr(target_os = "linux", path = "multi_server.rs")]
#[cfg_attr(not(target_os = "linux"), path = "non_linux_multi_server.rs")]
pub mod multi_server;
