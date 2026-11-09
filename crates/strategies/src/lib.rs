//! Trading strategies: funding carry (primary), trend, Kelly, Bayesian, risk gates.

pub mod bayesian;
pub mod conviction;
pub mod filters;
pub mod funding;
pub mod kelly;
pub mod majors_alpha;
pub mod protections;
pub mod risk_gate;
pub mod signals;

pub use conviction::*;
pub use protections::*;

pub use bayesian::*;
pub use filters::*;
pub use funding::*;
pub use kelly::*;
pub use majors_alpha::*;
pub use risk_gate::*;
pub use signals::*;
