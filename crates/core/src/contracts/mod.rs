//! Serde types for every file format and IPC payload in docs/CONTRACTS.md.
//!
//! Since M0.3 these types are the source of truth: docs/CONTRACTS.md, `ui/types.d.ts` and the
//! generated examples in `ui-dev/fixtures/contracts/` follow them (DEVELOPMENT.md §4.7).
//!
//! Conventions (CONTRACTS.md §1):
//! - Keys are `snake_case`. Unknown extra fields are ignored on read.
//! - Optional values are written as `null`, never omitted (IPC requests may omit fields where the
//!   command says "omitted = unchanged").
//! - Times are [`Timestamp`]s: RFC 3339 UTC with `Z` at second precision.
//! - Top-level files implement [`VersionedFile`]. Read them with [`parse_versioned`], which rejects
//!   an unknown `schema_version` before looking at anything else.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Declares an enum whose variants serialize as the given string literals. `ALL` lists the variants
/// in declaration order, `as_str` returns the literal, and `Display` prints it.
macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident = $value:literal, )+
        }
    ) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
            serde::Serialize, serde::Deserialize,
        )]
        pub enum $name {
            $( $(#[$vmeta])* #[serde(rename = $value)] $variant, )+
        }

        impl $name {
            /// Every value, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The value as written in JSON.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value,)+
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

mod acquisition;
mod case;
mod enums;
pub mod examples;
mod ipc;
mod run;
mod timestamp;
mod tools;

#[cfg(test)]
mod tests;

pub use acquisition::*;
pub use case::*;
pub use enums::*;
pub use ipc::*;
pub use run::*;
pub use timestamp::*;
pub use tools::*;

/// A top-level JSON file with a `schema_version` (CONTRACTS.md §1).
pub trait VersionedFile: Serialize + DeserializeOwned {
    /// The file name used in error messages, e.g. `run.json`.
    const FILE: &'static str;
    /// The only `schema_version` this build reads and writes.
    const SCHEMA_VERSION: u32 = 1;
}

/// Errors from reading contract data.
#[derive(Debug, thiserror::Error)]
pub enum ContractError {
    #[error("{file} has no integer schema_version")]
    MissingSchemaVersion { file: &'static str },
    #[error(
        "{file} has schema_version {found}; this version of suiteDFIR reads only version {supported}"
    )]
    UnsupportedSchemaVersion {
        file: &'static str,
        found: u64,
        supported: u32,
    },
    #[error("{file} is not valid: {source}")]
    Invalid {
        file: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid timestamp {0:?}: expected RFC 3339 UTC with Z at second precision")]
    InvalidTimestamp(String),
}

/// Parses a top-level file. A missing or unknown `schema_version` is rejected before any other
/// field is looked at, so a newer file fails with a version error rather than a field error.
pub fn parse_versioned<T: VersionedFile>(bytes: &[u8]) -> Result<T, ContractError> {
    #[derive(serde::Deserialize)]
    struct Probe {
        schema_version: Option<serde_json::Value>,
    }
    let invalid = |source| ContractError::Invalid {
        file: T::FILE,
        source,
    };
    let probe: Probe = serde_json::from_slice(bytes).map_err(invalid)?;
    let found = probe
        .schema_version
        .as_ref()
        .and_then(serde_json::Value::as_u64)
        .ok_or(ContractError::MissingSchemaVersion { file: T::FILE })?;
    if found != u64::from(T::SCHEMA_VERSION) {
        return Err(ContractError::UnsupportedSchemaVersion {
            file: T::FILE,
            found,
            supported: T::SCHEMA_VERSION,
        });
    }
    serde_json::from_slice(bytes).map_err(invalid)
}
