#[derive(Debug, Clone, Copy)]
pub struct MockPubSub {}

impl MockPubSub {
    pub const fn new() -> Self {
        Self {}
    }
}

impl Default for MockPubSub {
    fn default() -> Self {
        Self::new()
    }
}
