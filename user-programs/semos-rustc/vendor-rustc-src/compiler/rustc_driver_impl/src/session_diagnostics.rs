#[cfg(target_os = "none")] use alloc::{boxed::Box, string::{String, ToString}, vec::Vec, borrow::ToOwned};
#[cfg(not(target_os = "none"))] use std::error::Error;
#[cfg(target_os = "none")] use core::error::Error;
#[cfg(not(target_os = "none"))] use std::{io, path::{Path, PathBuf}};
#[cfg(target_os = "none")] use semos_std::{io, path::{Path, PathBuf}};

use rustc_macros::{Diagnostic, Subdiagnostic};

#[derive(Diagnostic)]
#[diag(driver_impl_cant_emit_mir)]
pub struct CantEmitMIR {
    pub error: io::Error,
}

#[derive(Diagnostic)]
#[diag(driver_impl_rlink_unable_to_read)]
pub(crate) struct RlinkUnableToRead {
    pub err: io::Error,
}

#[derive(Diagnostic)]
#[diag(driver_impl_rlink_wrong_file_type)]
pub(crate) struct RLinkWrongFileType;

#[derive(Diagnostic)]
#[diag(driver_impl_rlink_empty_version_number)]
pub(crate) struct RLinkEmptyVersionNumber;

#[derive(Diagnostic)]
#[diag(driver_impl_rlink_encoding_version_mismatch)]
pub(crate) struct RLinkEncodingVersionMismatch {
    pub version_array: String,
    pub rlink_version: u32,
}

#[derive(Diagnostic)]
#[diag(driver_impl_rlink_rustc_version_mismatch)]
pub(crate) struct RLinkRustcVersionMismatch<'a> {
    pub rustc_version: String,
    pub current_version: &'a str,
}

#[derive(Diagnostic)]
#[diag(driver_impl_rlink_no_a_file)]
pub(crate) struct RlinkNotAFile;

#[derive(Diagnostic)]
#[diag(driver_impl_rlink_corrupt_file)]
pub(crate) struct RlinkCorruptFile<'a> {
    pub file: &'a Path,
}

#[derive(Diagnostic)]
#[diag(driver_impl_ice)]
pub(crate) struct Ice;

#[derive(Diagnostic)]
#[diag(driver_impl_ice_bug_report)]
pub(crate) struct IceBugReport<'a> {
    pub bug_report_url: &'a str,
}

#[derive(Diagnostic)]
#[diag(driver_impl_ice_bug_report_update_note)]
pub(crate) struct UpdateNightlyNote;

#[derive(Diagnostic)]
#[diag(driver_impl_ice_bug_report_internal_feature)]
pub(crate) struct IceBugReportInternalFeature;

#[derive(Diagnostic)]
#[diag(driver_impl_ice_version)]
pub(crate) struct IceVersion<'a> {
    pub version: &'a str,
    pub triple: &'a str,
}

#[derive(Diagnostic)]
#[diag(driver_impl_ice_path)]
pub(crate) struct IcePath {
    pub path: PathBuf,
}

#[derive(Diagnostic)]
#[diag(driver_impl_ice_path_error)]
pub(crate) struct IcePathError {
    pub path: PathBuf,
    pub error: String,
    #[subdiagnostic]
    pub env_var: Option<IcePathErrorEnv>,
}

#[derive(Subdiagnostic)]
#[note(driver_impl_ice_path_error_env)]
pub(crate) struct IcePathErrorEnv {
    pub env_var: PathBuf,
}

#[derive(Diagnostic)]
#[diag(driver_impl_ice_flags)]
pub(crate) struct IceFlags {
    pub flags: String,
}

#[derive(Diagnostic)]
#[diag(driver_impl_ice_exclude_cargo_defaults)]
pub(crate) struct IceExcludeCargoDefaults;

#[derive(Diagnostic)]
#[diag(driver_impl_unstable_feature_usage)]
pub(crate) struct UnstableFeatureUsage {
    pub error: Box<dyn Error>,
}
