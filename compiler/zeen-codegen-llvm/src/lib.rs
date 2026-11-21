mod codegen;
mod error;

pub use codegen::{CodeGen, CodeGenNames, CodegenOptions, CodegenUnits};
pub use error::CodegenError;

#[cfg(test)]
mod fixtures;

#[cfg(test)]
mod tests;
