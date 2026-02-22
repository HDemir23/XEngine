use crate::arbitrage::detector::ArbOpportunity;
use crate::arbitrage::graph::ArbPath;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Strategy {
    Conservative,
    #[default]
    Moderate,
    Aggressive,
}

impl Strategy {
    pub fn filter(&self, opp: &ArbOpportunity) -> bool {
        match self {
            Strategy::Conservative => {
                opp.net_profit_pct > 3.0 && opp.confidence > 0.85
            }
            Strategy::Moderate => {
                opp.net_profit_pct > 1.5 && opp.confidence > 0.70
            }
            Strategy::Aggressive => {
                opp.net_profit_pct > 0.5
            }
        }
    }

    #[allow(dead_code)]
    pub fn filter_path(&self, path: &ArbPath) -> bool {
        match self {
            Strategy::Conservative => {
                path.net_profit_pct > 3.0 && path.confidence > 0.85
            }
            Strategy::Moderate => {
                path.net_profit_pct > 1.5 && path.confidence > 0.70
            }
            Strategy::Aggressive => {
                path.net_profit_pct > 0.5
            }
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Strategy::Conservative => "conservative",
            Strategy::Moderate => "moderate",
            Strategy::Aggressive => "aggressive",
        }
    }
}
