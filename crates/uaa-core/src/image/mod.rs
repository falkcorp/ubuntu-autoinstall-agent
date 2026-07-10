// file: crates/uaa-core/src/image/mod.rs
// version: 1.0.1
// guid: k1l2m3n4-o5p6-7890-1234-567890klmnop

//! Image management module for Ubuntu AutoInstall Agent

pub mod builder;
pub mod customizer;
pub mod deployer;
pub mod manager;

pub use builder::ImageBuilder;
pub use customizer::ImageCustomizer;
pub use deployer::ImageDeployer;
pub use manager::ImageManager;
