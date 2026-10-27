// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
//! Archive extraction that rejects links and traversal before any write.
use std::collections::BTreeMap;
use std::io::Cursor;
use std::io::Read;

use flate2::read::GzDecoder;

use crate::error_context::ErrorContext;
use crate::required::Required;
use crate::tools_error::Result;
use crate::tools_error::config;
use crate::util::safe_relative;
const LIMIT: u64 = 512 * 1024 * 1024;
pub(crate) fn unpack(bytes: &[u8], format: &str) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    let mut size = 0;
    if format == "tar.gz" {
        let mut archive = tar::Archive::new(GzDecoder::new(Cursor::new(bytes)));
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_str().required("archive path is not UTF-8")?.to_owned();
            let clean = path.trim_end_matches('/');
            safe_relative(clean)?;
            let kind = entry.header().entry_type();
            if kind.is_dir() {
                continue;
            }
            if !kind.is_file() {
                return Err(config(format!("unsupported archive entry: {path}")));
            }
            let length = entry.size();
            size += length;
            if size > LIMIT {
                return Err(config("archive exceeds size limit"));
            }
            let mut contents = Vec::new();
            entry.read_to_end(&mut contents)?;
            if files.insert(clean.to_owned(), contents).is_some() {
                return Err(config("duplicate archive path"));
            }
        }
    } else if format == "zip" {
        let mut archive =
            zip::ZipArchive::new(Cursor::new(bytes)).configuration_context(format_args!("zip archive"))?;
        if archive.has_duplicate_names() {
            return Err(config("duplicate archive path"));
        }
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).configuration_display()?;
            let name = entry.name().configuration_display()?.trim_end_matches('/').to_owned();
            safe_relative(&name)?;
            if let Some(mode) = entry.unix_mode() {
                let kind = mode & 0o170000;
                if kind != 0 && kind != 0o100000 && kind != 0o040000 {
                    return Err(config("unsupported zip entry type"));
                }
            }
            if entry.is_dir() {
                continue;
            }
            size += entry.size();
            if size > LIMIT {
                return Err(config("archive exceeds size limit"));
            }
            let mut contents = Vec::new();
            entry.read_to_end(&mut contents)?;
            if files.insert(name, contents).is_some() {
                return Err(config("duplicate archive path"));
            }
        }
    } else {
        return Err(config(format!("unsupported archive format: {format}")));
    }
    Ok(files)
}
