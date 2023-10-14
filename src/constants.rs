#[cfg(debug_assertions)]
pub const MAX_TURNS: u64 = 25;

#[cfg(not(debug_assertions))]
pub const MAX_TURNS: u64 = 125;

// be careful with this, these threads are BLOCKING the event loop, they don't yield for performance reasons
#[cfg(not(debug_assertions))]
pub const THREAD_COUNT: usize = 15;
#[cfg(debug_assertions)]
pub const THREAD_COUNT: usize = 1;

pub const THINKING_TIME: u64=300;

pub const GIO_ENDPOINT: &str = "wss://botws.generals.io/socket.io/?EIO=4&transport=websocket";
// pub const GIO_ENDPOINT: &str = "wss://generals.io/socket.io/?EIO=4&transport=websocket";

pub fn load_env_vars() -> (String, String, Option<String>) {
    // first read dotenv
    dotenv::dotenv().ok();

    // return userid, username and game id
    let userid = std::env::var("USERID").expect("USERID env var not set");
    let username = std::env::var("USERNAME").expect("USERNAME env var not set");
    let gameid = std::env::var("GAMEID").ok();

    (userid, username, gameid)
}
