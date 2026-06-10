//! # Rust Compiler Self-Profiling
//!
//! M27 Phase 2a A1 + R4 B2: gated to host. The SemOS-target build
//! ships a no-op shim that preserves the full public surface
//! (SelfProfilerRef / SelfProfiler / TimingGuard / VerboseTimingGuard
//! / EventArgRecorder / QueryInvocationId / TimePassesFormat /
//! EventId) but never records anything. The measureme crate, which
//! does mmap-backed event streaming, is not on the SemOS port path.

#[cfg(not(target_os = "none"))]
mod imp_std {
    // ----- BEGIN UPSTREAM BODY (unchanged) -----

    use std::borrow::Borrow;
    use std::collections::hash_map::Entry;
    use std::error::Error;
    use std::fmt::Display;
    use std::intrinsics::unlikely;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};
    use std::{fs, process};

    pub use measureme::EventId;
    use measureme::{EventIdBuilder, Profiler, SerializableString, StringId};
    use parking_lot::RwLock;
    use smallvec::SmallVec;
    use tracing::warn;

    use crate::fx::FxHashMap;
    use crate::outline;
    use crate::sync::AtomicU64;

    bitflags::bitflags! {
        #[derive(Clone, Copy)]
        struct EventFilter: u16 {
            const GENERIC_ACTIVITIES  = 1 << 0;
            const QUERY_PROVIDERS     = 1 << 1;
            const QUERY_CACHE_HITS    = 1 << 2;
            const QUERY_BLOCKED       = 1 << 3;
            const INCR_CACHE_LOADS    = 1 << 4;

            const QUERY_KEYS          = 1 << 5;
            const FUNCTION_ARGS       = 1 << 6;
            const LLVM                = 1 << 7;
            const INCR_RESULT_HASHING = 1 << 8;
            const ARTIFACT_SIZES      = 1 << 9;
            const QUERY_CACHE_HIT_COUNTS  = 1 << 10;

            const DEFAULT = Self::GENERIC_ACTIVITIES.bits() |
                            Self::QUERY_PROVIDERS.bits() |
                            Self::QUERY_BLOCKED.bits() |
                            Self::INCR_CACHE_LOADS.bits() |
                            Self::INCR_RESULT_HASHING.bits() |
                            Self::ARTIFACT_SIZES.bits() |
                            Self::QUERY_CACHE_HIT_COUNTS.bits();

            const ARGS = Self::QUERY_KEYS.bits() | Self::FUNCTION_ARGS.bits();
            const QUERY_CACHE_HIT_COMBINED = Self::QUERY_CACHE_HITS.bits() | Self::QUERY_CACHE_HIT_COUNTS.bits();
        }
    }

    const EVENT_FILTERS_BY_NAME: &[(&str, EventFilter)] = &[
        ("none", EventFilter::empty()),
        ("all", EventFilter::all()),
        ("default", EventFilter::DEFAULT),
        ("generic-activity", EventFilter::GENERIC_ACTIVITIES),
        ("query-provider", EventFilter::QUERY_PROVIDERS),
        ("query-cache-hit", EventFilter::QUERY_CACHE_HITS),
        ("query-cache-hit-count", EventFilter::QUERY_CACHE_HIT_COUNTS),
        ("query-blocked", EventFilter::QUERY_BLOCKED),
        ("incr-cache-load", EventFilter::INCR_CACHE_LOADS),
        ("query-keys", EventFilter::QUERY_KEYS),
        ("function-args", EventFilter::FUNCTION_ARGS),
        ("args", EventFilter::ARGS),
        ("llvm", EventFilter::LLVM),
        ("incr-result-hashing", EventFilter::INCR_RESULT_HASHING),
        ("artifact-sizes", EventFilter::ARTIFACT_SIZES),
    ];

    pub struct QueryInvocationId(pub u32);

    #[derive(Clone, Copy, PartialEq, Hash, Debug)]
    pub enum TimePassesFormat {
        Text,
        Json,
    }

    #[derive(Clone)]
    pub struct SelfProfilerRef {
        profiler: Option<Arc<SelfProfiler>>,
        event_filter_mask: EventFilter,
        print_verbose_generic_activities: Option<TimePassesFormat>,
    }

    impl SelfProfilerRef {
        pub fn new(
            profiler: Option<Arc<SelfProfiler>>,
            print_verbose_generic_activities: Option<TimePassesFormat>,
        ) -> SelfProfilerRef {
            let event_filter_mask =
                profiler.as_ref().map_or(EventFilter::empty(), |p| p.event_filter_mask);

            SelfProfilerRef { profiler, event_filter_mask, print_verbose_generic_activities }
        }

        #[inline(always)]
        fn exec<F>(&self, event_filter: EventFilter, f: F) -> TimingGuard<'_>
        where
            F: for<'a> FnOnce(&'a SelfProfiler) -> TimingGuard<'a>,
        {
            #[inline(never)]
            #[cold]
            fn cold_call<F>(profiler_ref: &SelfProfilerRef, f: F) -> TimingGuard<'_>
            where
                F: for<'a> FnOnce(&'a SelfProfiler) -> TimingGuard<'a>,
            {
                let profiler = profiler_ref.profiler.as_ref().unwrap();
                f(profiler)
            }

            if self.event_filter_mask.contains(event_filter) {
                cold_call(self, f)
            } else {
                TimingGuard::none()
            }
        }

        pub fn verbose_generic_activity(&self, event_label: &'static str) -> VerboseTimingGuard<'_> {
            let message_and_format =
                self.print_verbose_generic_activities.map(|format| (event_label.to_owned(), format));

            VerboseTimingGuard::start(message_and_format, self.generic_activity(event_label))
        }

        pub fn verbose_generic_activity_with_arg<A>(
            &self,
            event_label: &'static str,
            event_arg: A,
        ) -> VerboseTimingGuard<'_>
        where
            A: Borrow<str> + Into<String>,
        {
            let message_and_format = self
                .print_verbose_generic_activities
                .map(|format| (format!("{}({})", event_label, event_arg.borrow()), format));

            VerboseTimingGuard::start(
                message_and_format,
                self.generic_activity_with_arg(event_label, event_arg),
            )
        }

        #[inline(always)]
        pub fn generic_activity(&self, event_label: &'static str) -> TimingGuard<'_> {
            self.exec(EventFilter::GENERIC_ACTIVITIES, |profiler| {
                let event_label = profiler.get_or_alloc_cached_string(event_label);
                let event_id = EventId::from_label(event_label);
                TimingGuard::start(profiler, profiler.generic_activity_event_kind, event_id)
            })
        }

        #[inline(always)]
        pub fn generic_activity_with_event_id(&self, event_id: EventId) -> TimingGuard<'_> {
            self.exec(EventFilter::GENERIC_ACTIVITIES, |profiler| {
                TimingGuard::start(profiler, profiler.generic_activity_event_kind, event_id)
            })
        }

        #[inline(always)]
        pub fn generic_activity_with_arg<A>(
            &self,
            event_label: &'static str,
            event_arg: A,
        ) -> TimingGuard<'_>
        where
            A: Borrow<str> + Into<String>,
        {
            self.exec(EventFilter::GENERIC_ACTIVITIES, |profiler| {
                let builder = EventIdBuilder::new(&profiler.profiler);
                let event_label = profiler.get_or_alloc_cached_string(event_label);
                let event_id = if profiler.event_filter_mask.contains(EventFilter::FUNCTION_ARGS) {
                    let event_arg = profiler.get_or_alloc_cached_string(event_arg);
                    builder.from_label_and_arg(event_label, event_arg)
                } else {
                    builder.from_label(event_label)
                };
                TimingGuard::start(profiler, profiler.generic_activity_event_kind, event_id)
            })
        }

        #[inline(always)]
        pub fn generic_activity_with_arg_recorder<F>(
            &self,
            event_label: &'static str,
            mut f: F,
        ) -> TimingGuard<'_>
        where
            F: FnMut(&mut EventArgRecorder<'_>),
        {
            self.exec(EventFilter::GENERIC_ACTIVITIES, |profiler| {
                let builder = EventIdBuilder::new(&profiler.profiler);
                let event_label = profiler.get_or_alloc_cached_string(event_label);

                let event_id = if profiler.event_filter_mask.contains(EventFilter::FUNCTION_ARGS) {
                    let mut recorder = EventArgRecorder { profiler, args: SmallVec::new() };
                    f(&mut recorder);

                    if recorder.args.is_empty() {
                        panic!(
                            "The closure passed to `generic_activity_with_arg_recorder` needs to \
                             record at least one argument"
                        );
                    }

                    builder.from_label_and_args(event_label, &recorder.args)
                } else {
                    builder.from_label(event_label)
                };
                TimingGuard::start(profiler, profiler.generic_activity_event_kind, event_id)
            })
        }

        #[inline(always)]
        pub fn artifact_size<A>(&self, artifact_kind: &str, artifact_name: A, size: u64)
        where
            A: Borrow<str> + Into<String>,
        {
            drop(self.exec(EventFilter::ARTIFACT_SIZES, |profiler| {
                let builder = EventIdBuilder::new(&profiler.profiler);
                let event_label = profiler.get_or_alloc_cached_string(artifact_kind);
                let event_arg = profiler.get_or_alloc_cached_string(artifact_name);
                let event_id = builder.from_label_and_arg(event_label, event_arg);
                let thread_id = get_thread_id();

                profiler.profiler.record_integer_event(
                    profiler.artifact_size_event_kind,
                    event_id,
                    thread_id,
                    size,
                );

                TimingGuard::none()
            }))
        }

        #[inline(always)]
        pub fn generic_activity_with_args(
            &self,
            event_label: &'static str,
            event_args: &[String],
        ) -> TimingGuard<'_> {
            self.exec(EventFilter::GENERIC_ACTIVITIES, |profiler| {
                let builder = EventIdBuilder::new(&profiler.profiler);
                let event_label = profiler.get_or_alloc_cached_string(event_label);
                let event_id = if profiler.event_filter_mask.contains(EventFilter::FUNCTION_ARGS) {
                    let event_args: Vec<_> = event_args
                        .iter()
                        .map(|s| profiler.get_or_alloc_cached_string(&s[..]))
                        .collect();
                    builder.from_label_and_args(event_label, &event_args)
                } else {
                    builder.from_label(event_label)
                };
                TimingGuard::start(profiler, profiler.generic_activity_event_kind, event_id)
            })
        }

        #[inline(always)]
        pub fn query_provider(&self) -> TimingGuard<'_> {
            self.exec(EventFilter::QUERY_PROVIDERS, |profiler| {
                TimingGuard::start(profiler, profiler.query_event_kind, EventId::INVALID)
            })
        }

        #[inline(always)]
        pub fn query_cache_hit(&self, query_invocation_id: QueryInvocationId) {
            #[inline(never)]
            #[cold]
            fn cold_call(profiler_ref: &SelfProfilerRef, query_invocation_id: QueryInvocationId) {
                if profiler_ref.event_filter_mask.contains(EventFilter::QUERY_CACHE_HIT_COUNTS) {
                    profiler_ref
                        .profiler
                        .as_ref()
                        .unwrap()
                        .increment_query_cache_hit_counters(QueryInvocationId(query_invocation_id.0));
                }
                if unlikely(profiler_ref.event_filter_mask.contains(EventFilter::QUERY_CACHE_HITS)) {
                    profiler_ref.instant_query_event(
                        |profiler| profiler.query_cache_hit_event_kind,
                        query_invocation_id,
                    );
                }
            }

            if unlikely(self.event_filter_mask.intersects(EventFilter::QUERY_CACHE_HIT_COMBINED)) {
                cold_call(self, query_invocation_id);
            }
        }

        #[inline(always)]
        pub fn query_blocked(&self) -> TimingGuard<'_> {
            self.exec(EventFilter::QUERY_BLOCKED, |profiler| {
                TimingGuard::start(profiler, profiler.query_blocked_event_kind, EventId::INVALID)
            })
        }

        #[inline(always)]
        pub fn incr_cache_loading(&self) -> TimingGuard<'_> {
            self.exec(EventFilter::INCR_CACHE_LOADS, |profiler| {
                TimingGuard::start(
                    profiler,
                    profiler.incremental_load_result_event_kind,
                    EventId::INVALID,
                )
            })
        }

        #[inline(always)]
        pub fn incr_result_hashing(&self) -> TimingGuard<'_> {
            self.exec(EventFilter::INCR_RESULT_HASHING, |profiler| {
                TimingGuard::start(
                    profiler,
                    profiler.incremental_result_hashing_event_kind,
                    EventId::INVALID,
                )
            })
        }

        #[inline(always)]
        fn instant_query_event(
            &self,
            event_kind: fn(&SelfProfiler) -> StringId,
            query_invocation_id: QueryInvocationId,
        ) {
            let event_id = StringId::new_virtual(query_invocation_id.0);
            let thread_id = get_thread_id();
            let profiler = self.profiler.as_ref().unwrap();
            profiler.profiler.record_instant_event(
                event_kind(profiler),
                EventId::from_virtual(event_id),
                thread_id,
            );
        }

        pub fn with_profiler(&self, f: impl FnOnce(&SelfProfiler)) {
            if let Some(profiler) = &self.profiler {
                f(profiler)
            }
        }

        pub fn get_or_alloc_cached_string(&self, s: &str) -> Option<StringId> {
            self.profiler.as_ref().map(|p| p.get_or_alloc_cached_string(s))
        }

        pub fn store_query_cache_hits(&self) {
            if self.event_filter_mask.contains(EventFilter::QUERY_CACHE_HIT_COUNTS) {
                let profiler = self.profiler.as_ref().unwrap();
                let query_hits = profiler.query_hits.read();
                let builder = EventIdBuilder::new(&profiler.profiler);
                let thread_id = get_thread_id();
                for (query_invocation, hit_count) in query_hits.iter().enumerate() {
                    let hit_count = hit_count.load(Ordering::Relaxed);
                    if hit_count > 0 {
                        let event_id =
                            builder.from_label(StringId::new_virtual(query_invocation as u64));
                        profiler.profiler.record_integer_event(
                            profiler.query_cache_hit_count_event_kind,
                            event_id,
                            thread_id,
                            hit_count,
                        );
                    }
                }
            }
        }

        #[inline]
        pub fn enabled(&self) -> bool {
            self.profiler.is_some()
        }

        #[inline]
        pub fn llvm_recording_enabled(&self) -> bool {
            self.event_filter_mask.contains(EventFilter::LLVM)
        }
        #[inline]
        pub fn get_self_profiler(&self) -> Option<Arc<SelfProfiler>> {
            self.profiler.clone()
        }

        pub fn is_args_recording_enabled(&self) -> bool {
            self.enabled() && self.event_filter_mask.intersects(EventFilter::ARGS)
        }
    }

    pub struct EventArgRecorder<'p> {
        profiler: &'p SelfProfiler,
        args: SmallVec<[StringId; 2]>,
    }

    impl EventArgRecorder<'_> {
        pub fn record_arg<A>(&mut self, event_arg: A)
        where
            A: Borrow<str> + Into<String>,
        {
            let event_arg = self.profiler.get_or_alloc_cached_string(event_arg);
            self.args.push(event_arg);
        }
    }

    pub struct SelfProfiler {
        profiler: Profiler,
        event_filter_mask: EventFilter,

        string_cache: RwLock<FxHashMap<String, StringId>>,
        query_hits: RwLock<Vec<AtomicU64>>,

        query_event_kind: StringId,
        generic_activity_event_kind: StringId,
        incremental_load_result_event_kind: StringId,
        incremental_result_hashing_event_kind: StringId,
        query_blocked_event_kind: StringId,
        query_cache_hit_event_kind: StringId,
        artifact_size_event_kind: StringId,
        query_cache_hit_count_event_kind: StringId,
    }

    impl SelfProfiler {
        pub fn new(
            output_directory: &Path,
            crate_name: Option<&str>,
            event_filters: Option<&[String]>,
            counter_name: &str,
        ) -> Result<SelfProfiler, Box<dyn Error + Send + Sync>> {
            fs::create_dir_all(output_directory)?;

            let crate_name = crate_name.unwrap_or("unknown-crate");
            let pid: u32 = process::id();
            let filename = format!("{crate_name}-{pid:07}.rustc_profile");
            let path = output_directory.join(filename);
            let profiler =
                Profiler::with_counter(&path, measureme::counters::Counter::by_name(counter_name)?)?;

            let query_event_kind = profiler.alloc_string("Query");
            let generic_activity_event_kind = profiler.alloc_string("GenericActivity");
            let incremental_load_result_event_kind = profiler.alloc_string("IncrementalLoadResult");
            let incremental_result_hashing_event_kind =
                profiler.alloc_string("IncrementalResultHashing");
            let query_blocked_event_kind = profiler.alloc_string("QueryBlocked");
            let query_cache_hit_event_kind = profiler.alloc_string("QueryCacheHit");
            let artifact_size_event_kind = profiler.alloc_string("ArtifactSize");
            let query_cache_hit_count_event_kind = profiler.alloc_string("QueryCacheHitCount");

            let mut event_filter_mask = EventFilter::empty();

            if let Some(event_filters) = event_filters {
                let mut unknown_events = vec![];
                for item in event_filters {
                    if let Some(&(_, mask)) =
                        EVENT_FILTERS_BY_NAME.iter().find(|&(name, _)| name == item)
                    {
                        event_filter_mask |= mask;
                    } else {
                        unknown_events.push(item.clone());
                    }
                }

                if !unknown_events.is_empty() {
                    unknown_events.sort();
                    unknown_events.dedup();

                    warn!(
                        "Unknown self-profiler events specified: {}. Available options are: {}.",
                        unknown_events.join(", "),
                        EVENT_FILTERS_BY_NAME
                            .iter()
                            .map(|&(name, _)| name.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            } else {
                event_filter_mask = EventFilter::DEFAULT;
            }

            Ok(SelfProfiler {
                profiler,
                event_filter_mask,
                string_cache: RwLock::new(FxHashMap::default()),
                query_event_kind,
                generic_activity_event_kind,
                incremental_load_result_event_kind,
                incremental_result_hashing_event_kind,
                query_blocked_event_kind,
                query_cache_hit_event_kind,
                artifact_size_event_kind,
                query_cache_hit_count_event_kind,
                query_hits: Default::default(),
            })
        }

        pub fn alloc_string<STR: SerializableString + ?Sized>(&self, s: &STR) -> StringId {
            self.profiler.alloc_string(s)
        }

        pub fn increment_query_cache_hit_counters(&self, id: QueryInvocationId) {
            let mut guard = self.query_hits.upgradable_read();
            let query_hits = &guard;
            let index = id.0 as usize;
            if index < query_hits.len() {
                query_hits[index].fetch_add(1, Ordering::Relaxed);
            } else {
                guard.with_upgraded(|vec| {
                    vec.resize_with(index + 1, || AtomicU64::new(0));
                    vec[index] = AtomicU64::from(1);
                });
            }
        }

        pub fn get_or_alloc_cached_string<A>(&self, s: A) -> StringId
        where
            A: Borrow<str> + Into<String>,
        {
            {
                let string_cache = self.string_cache.read();

                if let Some(&id) = string_cache.get(s.borrow()) {
                    return id;
                }
            }

            let mut string_cache = self.string_cache.write();
            match string_cache.entry(s.into()) {
                Entry::Occupied(e) => *e.get(),
                Entry::Vacant(e) => {
                    let string_id = self.profiler.alloc_string(&e.key()[..]);
                    *e.insert(string_id)
                }
            }
        }

        pub fn map_query_invocation_id_to_string(&self, from: QueryInvocationId, to: StringId) {
            let from = StringId::new_virtual(from.0);
            self.profiler.map_virtual_to_concrete_string(from, to);
        }

        pub fn bulk_map_query_invocation_id_to_single_string<I>(&self, from: I, to: StringId)
        where
            I: Iterator<Item = QueryInvocationId> + ExactSizeIterator,
        {
            let from = from.map(|qid| StringId::new_virtual(qid.0));
            self.profiler.bulk_map_virtual_to_single_concrete_string(from, to);
        }

        pub fn query_key_recording_enabled(&self) -> bool {
            self.event_filter_mask.contains(EventFilter::QUERY_KEYS)
        }

        pub fn event_id_builder(&self) -> EventIdBuilder<'_> {
            EventIdBuilder::new(&self.profiler)
        }
    }

    #[must_use]
    pub struct TimingGuard<'a>(Option<measureme::TimingGuard<'a>>);

    impl<'a> TimingGuard<'a> {
        #[inline]
        pub fn start(
            profiler: &'a SelfProfiler,
            event_kind: StringId,
            event_id: EventId,
        ) -> TimingGuard<'a> {
            let thread_id = get_thread_id();
            let raw_profiler = &profiler.profiler;
            let timing_guard =
                raw_profiler.start_recording_interval_event(event_kind, event_id, thread_id);
            TimingGuard(Some(timing_guard))
        }

        #[inline]
        pub fn finish_with_query_invocation_id(self, query_invocation_id: QueryInvocationId) {
            if let Some(guard) = self.0 {
                outline(|| {
                    let event_id = StringId::new_virtual(query_invocation_id.0);
                    let event_id = EventId::from_virtual(event_id);
                    guard.finish_with_override_event_id(event_id);
                });
            }
        }

        #[inline]
        pub fn none() -> TimingGuard<'a> {
            TimingGuard(None)
        }

        #[inline(always)]
        pub fn run<R>(self, f: impl FnOnce() -> R) -> R {
            let _timer = self;
            f()
        }
    }

    struct VerboseInfo {
        start_time: Instant,
        start_rss: Option<usize>,
        message: String,
        format: TimePassesFormat,
    }

    #[must_use]
    pub struct VerboseTimingGuard<'a> {
        info: Option<VerboseInfo>,
        _guard: TimingGuard<'a>,
    }

    impl<'a> VerboseTimingGuard<'a> {
        pub fn start(
            message_and_format: Option<(String, TimePassesFormat)>,
            _guard: TimingGuard<'a>,
        ) -> Self {
            VerboseTimingGuard {
                _guard,
                info: message_and_format.map(|(message, format)| VerboseInfo {
                    start_time: Instant::now(),
                    start_rss: get_resident_set_size(),
                    message,
                    format,
                }),
            }
        }

        #[inline(always)]
        pub fn run<R>(self, f: impl FnOnce() -> R) -> R {
            let _timer = self;
            f()
        }
    }

    impl Drop for VerboseTimingGuard<'_> {
        fn drop(&mut self) {
            if let Some(info) = &self.info {
                let end_rss = get_resident_set_size();
                let dur = info.start_time.elapsed();
                print_time_passes_entry(&info.message, dur, info.start_rss, end_rss, info.format);
            }
        }
    }

    struct JsonTimePassesEntry<'a> {
        pass: &'a str,
        time: f64,
        start_rss: Option<usize>,
        end_rss: Option<usize>,
    }

    impl Display for JsonTimePassesEntry<'_> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let Self { pass: what, time, start_rss, end_rss } = self;
            write!(f, r#"{{"pass":"{what}","time":{time},"rss_start":"#).unwrap();
            match start_rss {
                Some(rss) => write!(f, "{rss}")?,
                None => write!(f, "null")?,
            }
            write!(f, r#","rss_end":"#)?;
            match end_rss {
                Some(rss) => write!(f, "{rss}")?,
                None => write!(f, "null")?,
            }
            write!(f, "}}")?;
            Ok(())
        }
    }

    pub fn print_time_passes_entry(
        what: &str,
        dur: Duration,
        start_rss: Option<usize>,
        end_rss: Option<usize>,
        format: TimePassesFormat,
    ) {
        match format {
            TimePassesFormat::Json => {
                let entry =
                    JsonTimePassesEntry { pass: what, time: dur.as_secs_f64(), start_rss, end_rss };

                eprintln!(r#"time: {entry}"#);
                return;
            }
            TimePassesFormat::Text => (),
        }

        let is_notable = || {
            if dur.as_millis() > 5 {
                return true;
            }

            if let (Some(start_rss), Some(end_rss)) = (start_rss, end_rss) {
                let change_rss = end_rss.abs_diff(start_rss);
                if change_rss > 0 {
                    return true;
                }
            }

            false
        };
        if !is_notable() {
            return;
        }

        let rss_to_mb = |rss| (rss as f64 / 1_000_000.0).round() as usize;
        let rss_change_to_mb = |rss| (rss as f64 / 1_000_000.0).round() as i128;

        let mem_string = match (start_rss, end_rss) {
            (Some(start_rss), Some(end_rss)) => {
                let change_rss = end_rss as i128 - start_rss as i128;

                format!(
                    "; rss: {:>4}MB -> {:>4}MB ({:>+5}MB)",
                    rss_to_mb(start_rss),
                    rss_to_mb(end_rss),
                    rss_change_to_mb(change_rss),
                )
            }
            (Some(start_rss), None) => format!("; rss start: {:>4}MB", rss_to_mb(start_rss)),
            (None, Some(end_rss)) => format!("; rss end: {:>4}MB", rss_to_mb(end_rss)),
            (None, None) => String::new(),
        };

        eprintln!("time: {:>7}{}\t{}", duration_to_secs_str(dur), mem_string, what);
    }

    pub fn duration_to_secs_str(dur: std::time::Duration) -> String {
        format!("{:.3}", dur.as_secs_f64())
    }

    fn get_thread_id() -> u32 {
        std::thread::current().id().as_u64().get() as u32
    }

    cfg_select! {
        windows => {
            pub fn get_resident_set_size() -> Option<usize> {
                use windows::{
                    Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
                    Win32::System::Threading::GetCurrentProcess,
                };

                let mut pmc = PROCESS_MEMORY_COUNTERS::default();
                let pmc_size = size_of_val(&pmc);
                unsafe {
                    K32GetProcessMemoryInfo(
                        GetCurrentProcess(),
                        &mut pmc,
                        pmc_size as u32,
                    )
                }
                .ok()
                .ok()?;

                Some(pmc.WorkingSetSize)
            }
        }
        target_os = "macos" => {
            pub fn get_resident_set_size() -> Option<usize> {
                use libc::{c_int, c_void, getpid, proc_pidinfo, proc_taskinfo, PROC_PIDTASKINFO};
                use std::mem;
                const PROC_TASKINFO_SIZE: c_int = size_of::<proc_taskinfo>() as c_int;

                unsafe {
                    let mut info: proc_taskinfo = mem::zeroed();
                    let info_ptr = &mut info as *mut proc_taskinfo as *mut c_void;
                    let pid = getpid() as c_int;
                    let ret = proc_pidinfo(pid, PROC_PIDTASKINFO, 0, info_ptr, PROC_TASKINFO_SIZE);
                    if ret == PROC_TASKINFO_SIZE {
                        Some(info.pti_resident_size as usize)
                    } else {
                        None
                    }
                }
            }
        }
        unix => {
            pub fn get_resident_set_size() -> Option<usize> {
                let field = 1;
                let contents = fs::read("/proc/self/statm").ok()?;
                let contents = String::from_utf8(contents).ok()?;
                let s = contents.split_whitespace().nth(field)?;
                let npages = s.parse::<usize>().ok()?;
                Some(npages * 4096)
            }
        }
        _ => {
            pub fn get_resident_set_size() -> Option<usize> {
                None
            }
        }
    }

    #[cfg(test)]
    mod tests;
}

#[cfg(target_os = "none")]
mod imp_none {
    //! M27 R4 B2 — no-op self-profiler for the SemOS target. Public
    //! API matches upstream; all recording operations are immediate
    //! no-ops. measureme is not on the SemOS port path.

    use alloc::borrow::ToOwned;
    use alloc::boxed::Box;
    use alloc::string::String;
    use alloc::sync::Arc;
    use alloc::vec::Vec;
    use core::borrow::Borrow;
    // M27: align with semos_std's Duration so call sites in
    // rustc_codegen_ssa::base that use semos_std::time::Duration can
    // pass it directly. The no-op body doesn't read the value.
    use semos_std::time::Duration;

    pub struct QueryInvocationId(pub u32);

    /// Stub StringId: opaque interned-string handle in upstream measureme.
    /// All measureme operations are no-ops on SemOS, so the stub is unit-shaped.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct StringId;
    impl StringId {
        pub const INVALID: StringId = StringId;
        pub fn new_virtual(_id: u64) -> Self { StringId }
    }

    /// Stub StringComponent: enum used by measureme to build interned strings.
    #[derive(Clone, Copy, Debug)]
    pub enum StringComponent<'a> {
        Ref(StringId),
        Value(&'a str),
    }

    #[derive(Clone, Copy, PartialEq, Hash, Debug)]
    pub enum TimePassesFormat {
        Text,
        Json,
    }

    /// Stub EventId. Upstream is `measureme::EventId(u64)`. The SemOS
    /// stub is opaque; nothing inspects it because the profiler never
    /// records anything.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct EventId(u32);

    impl EventId {
        pub const INVALID: EventId = EventId(0);
        pub fn from_label(_label: u32) -> Self {
            EventId(0)
        }
        pub fn from_virtual(_id: u32) -> Self {
            EventId(0)
        }
    }

    /// Stub SelfProfiler. Holds nothing; matches upstream's
    /// constructor signature.
    pub struct SelfProfiler;

    impl SelfProfiler {
        pub fn new(
            _output_directory: &semos_std::path::Path,
            _crate_name: Option<&str>,
            _event_filters: Option<&[String]>,
            _counter_name: &str,
        ) -> Result<SelfProfiler, Box<dyn core::error::Error + Send + Sync>> {
            Ok(SelfProfiler)
        }

        pub fn query_key_recording_enabled(&self) -> bool {
            false
        }

        // No-op stubs used by rustc_query_impl::profiling_support.
        pub fn alloc_string<T: ?Sized>(&self, _: &T) -> StringId { StringId }
        pub fn event_id_builder(&self) -> EventIdBuilder<'_> { EventIdBuilder(core::marker::PhantomData) }
        pub fn get_or_alloc_cached_string<A>(&self, _: A) -> StringId { StringId }
        pub fn map_query_invocation_id_to_string(&self, _: QueryInvocationId, _: StringId) {}
        pub fn bulk_map_query_invocation_id_to_single_string<I>(&self, _: I, _: StringId) {}
    }

    /// Stub EventIdBuilder. Returns no-op EventIds.
    pub struct EventIdBuilder<'a>(core::marker::PhantomData<&'a ()>);
    impl<'a> EventIdBuilder<'a> {
        pub fn from_label(&self, _: StringId) -> EventId { EventId::INVALID }
        pub fn from_label_and_arg(&self, _: StringId, _: StringId) -> EventId { EventId::INVALID }
    }
    impl EventId {
        pub fn to_string_id(self) -> StringId { StringId }
    }

    #[derive(Clone, Default)]
    pub struct SelfProfilerRef {
        profiler: Option<Arc<SelfProfiler>>,
    }

    impl SelfProfilerRef {
        pub fn new(
            profiler: Option<Arc<SelfProfiler>>,
            _print_verbose_generic_activities: Option<TimePassesFormat>,
        ) -> SelfProfilerRef {
            SelfProfilerRef { profiler }
        }

        pub fn verbose_generic_activity(&self, _event_label: &'static str) -> VerboseTimingGuard<'_> {
            VerboseTimingGuard::none()
        }

        pub fn verbose_generic_activity_with_arg<A>(
            &self,
            _event_label: &'static str,
            _event_arg: A,
        ) -> VerboseTimingGuard<'_>
        where
            A: Borrow<str> + Into<String>,
        {
            VerboseTimingGuard::none()
        }

        #[inline(always)]
        pub fn generic_activity(&self, _event_label: &'static str) -> TimingGuard<'_> {
            TimingGuard::none()
        }

        #[inline(always)]
        pub fn generic_activity_with_event_id(&self, _event_id: EventId) -> TimingGuard<'_> {
            TimingGuard::none()
        }

        #[inline(always)]
        pub fn generic_activity_with_arg<A>(
            &self,
            _event_label: &'static str,
            _event_arg: A,
        ) -> TimingGuard<'_>
        where
            A: Borrow<str> + Into<String>,
        {
            TimingGuard::none()
        }

        #[inline(always)]
        pub fn generic_activity_with_arg_recorder<F>(
            &self,
            _event_label: &'static str,
            _f: F,
        ) -> TimingGuard<'_>
        where
            F: FnMut(&mut EventArgRecorder<'_>),
        {
            TimingGuard::none()
        }

        #[inline(always)]
        pub fn artifact_size<A>(&self, _artifact_kind: &str, _artifact_name: A, _size: u64)
        where
            A: Borrow<str> + Into<String>,
        {
        }

        #[inline(always)]
        pub fn generic_activity_with_args(
            &self,
            _event_label: &'static str,
            _event_args: &[String],
        ) -> TimingGuard<'_> {
            TimingGuard::none()
        }

        #[inline(always)]
        pub fn query_provider(&self) -> TimingGuard<'_> {
            TimingGuard::none()
        }

        #[inline(always)]
        pub fn query_cache_hit(&self, _query_invocation_id: QueryInvocationId) {}

        #[inline(always)]
        pub fn query_blocked(&self) -> TimingGuard<'_> {
            TimingGuard::none()
        }

        #[inline(always)]
        pub fn incr_cache_loading(&self) -> TimingGuard<'_> {
            TimingGuard::none()
        }

        #[inline(always)]
        pub fn incr_result_hashing(&self) -> TimingGuard<'_> {
            TimingGuard::none()
        }

        pub fn with_profiler(&self, f: impl FnOnce(&SelfProfiler)) {
            if let Some(profiler) = &self.profiler {
                f(profiler)
            }
        }

        pub fn get_or_alloc_cached_string(&self, _s: &str) -> Option<u32> {
            None
        }

        pub fn store_query_cache_hits(&self) {}

        #[inline]
        pub fn enabled(&self) -> bool {
            false
        }

        #[inline]
        pub fn llvm_recording_enabled(&self) -> bool {
            false
        }

        #[inline]
        pub fn get_self_profiler(&self) -> Option<Arc<SelfProfiler>> {
            self.profiler.clone()
        }

        pub fn is_args_recording_enabled(&self) -> bool {
            false
        }
    }

    pub struct EventArgRecorder<'p> {
        _marker: core::marker::PhantomData<&'p ()>,
    }

    impl EventArgRecorder<'_> {
        pub fn record_arg<A>(&mut self, _event_arg: A)
        where
            A: Borrow<str> + Into<String>,
        {
        }
    }

    #[must_use]
    pub struct TimingGuard<'a> {
        _marker: core::marker::PhantomData<&'a ()>,
    }

    impl<'a> TimingGuard<'a> {
        #[inline]
        pub fn finish_with_query_invocation_id(self, _qid: QueryInvocationId) {}

        #[inline]
        pub fn none() -> TimingGuard<'a> {
            TimingGuard { _marker: core::marker::PhantomData }
        }

        #[inline(always)]
        pub fn run<R>(self, f: impl FnOnce() -> R) -> R {
            f()
        }
    }

    #[must_use]
    pub struct VerboseTimingGuard<'a> {
        _marker: core::marker::PhantomData<&'a ()>,
    }

    impl<'a> VerboseTimingGuard<'a> {
        pub fn none() -> Self {
            VerboseTimingGuard { _marker: core::marker::PhantomData }
        }

        #[inline(always)]
        pub fn run<R>(self, f: impl FnOnce() -> R) -> R {
            f()
        }
    }

    pub fn print_time_passes_entry(
        _what: &str,
        _dur: Duration,
        _start_rss: Option<usize>,
        _end_rss: Option<usize>,
        _format: TimePassesFormat,
    ) {
    }

    pub fn duration_to_secs_str(dur: Duration) -> String {
        alloc::format!("{:.3}", dur.as_secs_f64())
    }

    pub fn get_resident_set_size() -> Option<usize> {
        None
    }

    // ToOwned re-export silences "unused import" warnings in callers
    // that drop into the no-op path.
    #[allow(dead_code)]
    fn _touch() -> alloc::string::String {
        "".to_owned()
    }

    impl QueryInvocationId {
        #[allow(dead_code)]
        const _USE: () = ();
    }

    // Ensure types lint clean even without callers.
    #[allow(dead_code)]
    fn _v(_v: Vec<u8>) {}
}

#[cfg(not(target_os = "none"))]
pub use imp_std::*;
#[cfg(target_os = "none")]
pub use imp_none::*;
