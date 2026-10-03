//! One integration-test binary for index-core, so the crate's dependency
//! tree links once instead of once per file.
mod excludes;
mod gitignore;
mod goto_definition_early_out;
mod index_build_bench;
mod nested_repositories;
mod perf_bench;
mod scope;
mod symbol_rank;
