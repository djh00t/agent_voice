//! Bounded value boundary for the OpenAI Realtime provider integration.
//!
//! The registered values are provider-independent and deterministic. Loading
//! this module performs no I/O, payload processing, or legacy integration;
//! later event and dispatch packages can build on the same side-effect-free
//! boundary.

/// Bounded, provider-independent Realtime values and redacted errors.
pub mod values;

#[cfg(test)]
mod tests {
    #[test]
    fn module_boundary_is_inert() {}
}
