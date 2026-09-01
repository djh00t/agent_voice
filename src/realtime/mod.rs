//! Bounded value boundary for the OpenAI Realtime provider integration.
//!
//! The registered values are provider-independent and deterministic. Loading
//! this module performs no I/O, payload processing, or legacy integration;
//! later event and dispatch packages can build on the same side-effect-free
//! boundary.

pub mod values;
pub mod client_events;
pub mod server_audio_events;
pub mod server_function_events;
pub mod server_response_events;
pub mod server_session_events;
pub mod events;

pub use events::{decode_server_event, RealtimeServerEvent};
pub use server_audio_events::RealtimeServerAudioEvent;
pub use server_function_events::RealtimeServerFunctionEvent;
pub use server_response_events::RealtimeServerResponseEvent;
pub use server_session_events::RealtimeServerSessionEvent;

#[cfg(test)]
mod tests {
    #[test]
    fn module_boundary_is_inert() {}
}
