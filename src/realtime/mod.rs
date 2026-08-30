//! Inert boundary for the future OpenAI Realtime provider integration.
//!
//! The bootstrap boundary deliberately performs no I/O, payload processing,
//! or legacy integration. Later packages may add value and dispatch modules
//! without changing this module's side-effect-free registration contract.

#[cfg(test)]
mod tests {
    #[test]
    fn module_boundary_is_inert() {}
}
