#[path = "../src/realtime/values.rs"]
pub mod values;

#[cfg(test)]
pub mod realtime {
    pub use crate::values;
}

#[path = "../src/realtime/server_function_events.rs"]
mod server_function_events;
