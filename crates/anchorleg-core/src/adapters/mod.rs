//! One adapter per vendor CLI. Each one turns its CLI's event stream into [`AgentSignal`]s, so
//! the supervisor never looks at a vendor's raw format.

pub mod agy;
pub mod claude;
pub mod codex;

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

/// What the supervisor needs to know from a running CLI, independent of vendor.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "signal", rename_all = "snake_case")]
pub enum AgentSignal {
    Started {
        session_id: String,
        model: Option<String>,
    },
    Quota(QuotaReading),
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
