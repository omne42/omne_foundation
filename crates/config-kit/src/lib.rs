#![forbid(unsafe_code)]

//! Shared configuration foundation primitives.
//!
//! This crate owns the parts of configuration handling that keep getting
//! reimplemented across repositories:
//!
//! - bounded, fail-closed, no-follow config file loading
//! - config format detection and typed parse/render helpers
//! - rooted candidate discovery and fail-closed path checks
//! - strict `${ENV_VAR}` interpolation
//! - recursive object-layer merge with change reporting
//! - higher-level typed schema loading for layered config files
//!
//! It deliberately does **not** own application-specific config schemas,
//! command-line contracts, or repository-local directory conventions.
//!
//! For business schemas that need "defaults + discovered file + optional env
//! interpolation + typed deserialize + explain", prefer
//! [`SchemaConfigLoader`]. It keeps schema ownership in the caller crate while
//! reusing shared config loading semantics.

mod env;
mod error;
mod format;
mod load;
mod merge;
mod schema;
mod typed;

pub use env::{
    EnvInterpolationOptions, interpolate_env_placeholders, interpolate_env_placeholders_with,
    is_valid_env_var_name,
};
pub use error::{Error, Result};
pub use format::ConfigFormat;
pub use load::{
    ConfigDocument, ConfigLoadOptions, DEFAULT_MAX_CONFIG_BYTES, HARD_MAX_CONFIG_BYTES,
    find_config_document, load_config_document, try_load_config_document,
};
pub use merge::{
    ConfigLayer, ConfigMergeStep, MergedConfig, merge_config_layers, merge_config_values,
    merge_config_values_in_place,
};
pub use schema::{
    LoadedSchemaConfig, LoadedSchemaLayer, SchemaConfigLoader, SchemaFileLayerOptions,
};
pub use typed::{
    ConfigFormatSet, load_typed_config_file, parse_typed_config_document,
    try_load_typed_config_file,
};
