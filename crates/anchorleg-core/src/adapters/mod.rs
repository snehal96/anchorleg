//! One adapter per vendor CLI. Each one turns its CLI's event stream into [`AgentSignal`]s, so
//! the supervisor never looks at a vendor's raw format.

pub mod agy;
pub mod claude;
pub mod codex;
pub mod codex_server;

use serde::Serialize;

/// Unix time in seconds.
pub type EpochSecs = i64;

/// Which quota window a reading is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Window {
    FiveHour,
    SevenDay,
    /// A window name we don't know yet, kept verbatim.
    Other(String),
}

impl Window {
    pub fn from_name(name: &str) -> Self {
        match name {
            "five_hour" | "5h" => Self::FiveHour,
            "seven_day" | "7d" | "weekly" => Self::SevenDay,
            other => Self::Other(other.to_owned()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::FiveHour => "five_hour",
            Self::SevenDay => "seven_day",
            Self::Other(name) => name,
        }
    }
}

/// Whether the vendor is still serving requests on this account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaStatus {
    Allowed,
    /// Still allowed, but past a warning threshold.
    Warning,
    Rejected,
    Other(String),
}

impl QuotaStatus {
    pub fn from_name(name: &str) -> Self {
        match name {
            "allowed" => Self::Allowed,
            "allowed_warning" | "warning" => Self::Warning,
            "rejected" => Self::Rejected,
            other => Self::Other(other.to_owned()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Allowed => "allowed",
            Self::Warning => "warning",
            Self::Rejected => "rejected",
            Self::Other(name) => name,
        }
    }
}

/// One quota reading. Every field is optional because CLIs often leave some out
/// (e.g. Claude omits utilization while the status is "allowed").
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuotaReading {
    pub window: Option<Window>,
    pub status: Option<QuotaStatus>,
    /// Fraction used, 0.0–1.0.
    pub used: Option<f64>,
    pub resets_at: Option<EpochSecs>,
}

/// Whether an account can keep working past its plan limits on paid usage credits (Claude's
/// "extra usage", Codex credits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CreditState {
    /// Not enabled, out of credits, or turned off by an admin (`reason` says which).
    Off,
    /// Credits would be used once a window runs out.
    Available,
    /// Requests are being paid from credits right now.
    InUse,
    /// The plan has no limit to run past.
    Unlimited,
}

impl CreditState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Available => "available",
            Self::InUse => "in_use",
            Self::Unlimited => "unlimited",
        }
    }

    pub fn from_name(name: &str) -> Self {
        match name {
            "available" => Self::Available,
            "in_use" => Self::InUse,
            "unlimited" => Self::Unlimited,
            _ => Self::Off,
        }
    }

    /// Requests can go on past the plan's limits.
    pub fn spendable(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// An account's usage-credit state, as the CLI last reported it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Credits {
    pub state: CreditState,
    /// Remaining balance when the CLI gives one (Codex), in the vendor's unit.
    pub balance: Option<f64>,
    /// Why credits are off, verbatim (e.g. `out_of_credits`, `org_level_disabled`).
    pub reason: Option<String>,
}

/// Tokens (and, when the CLI reports it, list-price cost) used by one launch, per model.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Usage {
    pub model: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// What the tokens would cost at API list prices (Claude reports it; others don't).
    pub cost_usd: Option<f64>,
}

impl Usage {
    pub fn is_empty(&self) -> bool {
        self.input_tokens == 0
            && self.output_tokens == 0
            && self.cache_read_tokens == 0
            && self.cache_write_tokens == 0
            && self.cost_usd.is_none_or(|c| c == 0.0)
    }
}

/// What the supervisor needs to know from a running CLI, independent of vendor.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "signal", rename_all = "snake_case")]
pub enum AgentSignal {
    Started {
        session_id: String,
        model: Option<String>,
    },
    Quota(QuotaReading),
    Credits(Credits),
    /// Sent once a launch's totals are known (usually with its result).
    Usage(Usage),
    /// The CLI is retrying a failed request; `category` is e.g. `rate_limit`.
    Retrying {
        category: Option<String>,
        attempt: Option<u32>,
    },
    /// The account can't serve more requests until `resets_at` (if known).
    LimitHit {
        window: Option<Window>,
        resets_at: Option<EpochSecs>,
    },
    /// `--resume` named a session this account can't see (e.g. it lives in another
    /// account's config dir).
    SessionNotFound,
    Finished {
        ok: bool,
        session_id: Option<String>,
        text: Option<String>,
    },
}
