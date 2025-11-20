pub mod dialect;
pub mod s;

pub use s::error::{SError, SResult};
pub use s::json::{json_to_sexpr, sexpr_to_json};
