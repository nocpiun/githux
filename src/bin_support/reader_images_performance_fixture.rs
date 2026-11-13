//! Original-byte stress fixture shared by combined view tests and native preview.
//! No live network, real credentials or third-party image redistribution.
// Preview and integration tests inspect different parts of the same fixture.
#![allow(dead_code)]
use crate::{
    data::reader_images::RepositoryImageService,
    domain::repository::{RepositoryIdentity, RepositoryLocator},
    fixture::{Pipeline, Scene},
    network::{
        cancel::CancellationToken,
        reader_images::{
            ExternalImageBytes, ExternalImageError, ExternalImageLimits, ExternalImageMetadata,
            ExternalImageResponse, ValidationHeaders,
        },
    },
    reader::ImageLimits,
    storage::cache::{CacheConfig, CacheScope, CacheStore},
};
use std::{
    pin::Pin,
    sync::{Arc, Mutex, OnceLock},
};

pub struct Originals {
    pub bytes: Arc<Vec<u8>>,
    pub reads: Mutex<Vec<String>>,
}
impl ExternalImageBytes for Originals {
    fn load_with_metadata<'a>(
        &'a self,
        url: &'a str,
        _: &'a ExternalImageLimits,
        _: ValidationHeaders<'a>,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ExternalImageResponse, ExternalImageError>> + Send + 'a>>
    {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(ExternalImageError::Cancelled);
            }
            let index = url
                .strip_prefix("https://originals.invalid/")
                .and_then(|s| s.strip_suffix(".png"))
                .and_then(|s| s.parse::<usize>().ok());
            assert!(
                index.is_some_and(|i| i < 9),
                "unmodelled original-image fixture source"
            );
            self.reads.lock().unwrap().push(url.to_string());
            Ok(ExternalImageResponse::Fetched {
                bytes: (*self.bytes).clone(),
                metadata: ExternalImageMetadata {
                    etag: Some("\"original\"".into()),
                    cache_control: Some("max-age=600".into()),
                    age_secs: Some(0),
                    ..Default::default()
                },
            })
        })
    }
}
fn originals(large: bool) -> Arc<Vec<u8>> {
    static LARGE: OnceLock<Arc<Vec<u8>>> = OnceLock::new();
    if !large {
        return Arc::new(crate::fixture::picture(128, 64));
    }
    LARGE
        .get_or_init(|| Arc::new(crate::fixture::picture(2559, 1520)))
        .clone()
}
pub fn markdown(table: bool) -> String {
    let mut text = "# Original screenshots\n\n".to_string();
    let detail =
        "Server activity, memory use, players, and live status for this screenshot. ".repeat(16);
    if table {
        text.push_str("| Left | Right |\n|:---:|:---:|\n");
        for row in 0..5 {
            let first = row * 2;
            let second = first + 1;
            let other = if second < 9 {
                format!("![Screenshot {second}](https://originals.invalid/{second}.png)")
            } else {
                String::new()
            };
            text.push_str(&format!("| ![Screenshot {first}](https://originals.invalid/{first}.png) | {other} |\n| Caption {first}. {detail} | Caption {second}. {detail} |\n"));
        }
    } else {
        for i in 0..9 {
            text.push_str(&format!(
                "![Screenshot {i}](https://originals.invalid/{i}.png)\n\nCaption {i}. {detail}\n\n"
            ));
        }
    }
    text
}
#[derive(Debug, serde::Serialize)]
pub struct ProcessMemory {
    pub working_set_bytes: u64,
    pub peak_working_set_bytes: u64,
    pub private_commit_bytes: u64,
}
#[cfg(not(windows))]
pub fn process_memory() -> Option<ProcessMemory> {
    None
}
#[cfg(windows)]
pub fn process_memory() -> Option<ProcessMemory> {
    #[repr(C)]
    struct Counters {
        cb: u32,
        faults: u32,
        peak_working: usize,
        working: usize,
        peak_paged: usize,
        paged: usize,
        peak_nonpaged: usize,
        nonpaged: usize,
        pagefile: usize,
        peak_pagefile: usize,
        private: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
    }
    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            counters: *mut Counters,
            size: u32,
        ) -> i32;
    }
    // PROCESS_MEMORY_COUNTERS_EX is an output-only Windows POD structure.
    let mut value: Counters = unsafe { std::mem::zeroed() };
    value.cb = std::mem::size_of::<Counters>() as u32;
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut value, value.cb) };
    (ok != 0).then_some(ProcessMemory {
        working_set_bytes: value.working as u64,
        peak_working_set_bytes: value.peak_working as u64,
        private_commit_bytes: value.private as u64,
    })
}

pub struct PerformanceFixture {
    pub service: Arc<RepositoryImageService>,
    pub source: Arc<Originals>,
    pub store: CacheStore,
    pub scope: CacheScope,
    pub _pipeline: Pipeline,
    _temp: tempfile::TempDir,
}
impl PerformanceFixture {
    pub fn new(large: bool, limits: ImageLimits) -> Self {
        let pipeline = Pipeline::new(Scene::Healthy);
        let temp = tempfile::tempdir().unwrap();
        let store = CacheStore::open(CacheConfig::at(temp.path().join("cache"))).unwrap();
        let session = pipeline.accounts().repo_service().session().unwrap();
        let scope = store
            .saved_account(session.context().auth().account.clone())
            .unwrap();
        let source = Arc::new(Originals {
            bytes: originals(large),
            reads: Mutex::default(),
        });
        let service = Arc::new(RepositoryImageService::with_cache(
            session,
            RepositoryIdentity {
                id: 42,
                locator: RepositoryLocator::new(crate::fixture::OWNER, crate::fixture::NAME)
                    .unwrap(),
            },
            pipeline.executor().clone(),
            source.clone(),
            limits,
            Some((store.clone(), scope.clone(), false)),
        ));
        Self {
            _pipeline: pipeline,
            _temp: temp,
            source,
            service,
            store,
            scope,
        }
    }
}
