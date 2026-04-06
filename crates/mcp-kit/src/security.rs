#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrustMode {
    /// Default: treat local config as untrusted and refuse "unsafe" actions
    /// such as spawning processes or connecting to arbitrary unix sockets.
    #[default]
    Untrusted,
    /// Fully trust local config and allow spawning processes / unix socket connects.
    Trusted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UntrustedStreamableHttpPolicy {
    /// When true (default), only allow `https://` URLs in untrusted mode.
    pub require_https: bool,
    /// Shared HTTP egress policy for host/IP/DNS restrictions in untrusted mode.
    pub outbound: http_kit::UntrustedOutboundPolicy,
}

impl Default for UntrustedStreamableHttpPolicy {
    fn default() -> Self {
        Self {
            require_https: true,
            outbound: http_kit::UntrustedOutboundPolicy::default(),
        }
    }
}
