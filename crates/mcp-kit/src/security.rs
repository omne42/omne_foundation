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
    /// When false (default), untrusted mode refuses arbitrary public hosts unless the target
    /// matches `outbound.allowed_hosts`.
    pub allow_public_hosts: bool,
    /// When false (default), untrusted mode rejects caller-provided custom HTTP headers.
    pub allow_custom_headers: bool,
    /// Shared HTTP egress policy for host/IP/DNS restrictions in untrusted mode.
    pub outbound: http_kit::UntrustedOutboundPolicy,
}

impl Default for UntrustedStreamableHttpPolicy {
    fn default() -> Self {
        Self {
            require_https: true,
            allow_public_hosts: false,
            allow_custom_headers: false,
            outbound: http_kit::UntrustedOutboundPolicy::default(),
        }
    }
}
