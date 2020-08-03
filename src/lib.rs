use aimp::{
    decoders::{
        AudioDecoder, AudioDecoderBuilder, AudioDecoderBuilderWrapper,
        AudioDecoderNotificationsWrapper, BufferingProgress, SampleFormat, StreamInfo,
    },
    file::{FileFormat, FileFormatWrapper, FileFormatsCategory, FileInfo},
    msg_box,
    stream::Stream,
    Plugin, PluginCategory, PluginInfo, CORE,
};
use log::LevelFilter;
use openmpt::module::{Logger, Module};
use pretty_env_logger::env_logger::WriteStyle;
use std::{
    mem,
    os::raw::c_double,
    sync::{Mutex, MutexGuard},
};

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("OpenMPT module")]
    Module,
}

const SAMPLE_RATE: i32 = 48000;

fn seconds_to_bytes(seconds: c_double) -> i64 {
    (seconds * SAMPLE_RATE as f64 * 2.0 * (32.0 / 8.0)) as i64
}

fn bytes_to_seconds(bytes: i64) -> c_double {
    bytes as f64 / (SAMPLE_RATE as f64 * 2.0 * (32.0 / 8.0))
}

struct ModuleWrapper(Module);

impl ModuleWrapper {
    fn ctl_set(&mut self, k: &str, v: &str) -> bool {
        self.0.ctl_set(k, v)
    }

    fn set_render_stereo_separation(&mut self, percentage: i32) -> bool {
        self.0.set_render_stereo_separation(percentage)
    }

    fn set_render_interpolation_filter_length(&mut self, filter_length: i32) -> bool {
        self.0.set_render_interpolation_filter_length(filter_length)
    }

    fn pos(&mut self) -> i64 {
        seconds_to_bytes(self.pos_secs())
    }

    fn pos_secs(&mut self) -> f64 {
        self.0.get_position_seconds()
    }

    fn set_pos_secs(&mut self, secs: f64) -> f64 {
        self.0.set_position_seconds(secs)
    }

    fn duration(&mut self) -> i64 {
        seconds_to_bytes(self.duration_secs())
    }

    fn duration_secs(&mut self) -> f64 {
        self.0.get_duration_seconds()
    }

    fn metadata(&mut self, key: &str) -> Option<String> {
        self.0.get_metadata(key).filter(|s| !s.is_empty())
    }

    fn read(&mut self, buf: &mut [u8]) -> i32 {
        let stereo_len = buf.len() / mem::size_of::<f32>();
        let mut stereo = unsafe {
            #[allow(clippy::cast_ptr_alignment)]
            Vec::from_raw_parts(
                buf as *mut [u8] as *mut u8 as *mut f32,
                stereo_len,
                stereo_len,
            )
        };

        let read = self
            .0
            .read_interleaved_float_stereo(SAMPLE_RATE, &mut stereo);

        mem::forget(stereo);

        (read * mem::size_of::<f32>() * 2) as i32
    }
}

struct OpenMptFormats;

impl FileFormat for OpenMptFormats {
    const DESCRIPTION: &'static str = "OpenMPT formats";
    const EXTS: &'static [&'static str] = &[
        "*.669", "*.amf", "*.ams", "*.c67", "*.dbm", "*.dtm", "*.far", "*.gdm", "*.ice", "*.st26",
        "*.imf", "*.it", "*.itp", "*.j2b", "*.m15", "*.stk", "*.mdl", "*.med", "*.mo3", "*.mod",
        "*.mptm", "*.mt2", "*.mtm", "*.okt", "*.oxm", "*.plm", "*.psm", "*.pt36", "*.ptm", "*.s3m",
        "*.sfx", "*.sfx2", "*.mms", "*.stm", "*.stp", "*.ult", "*.umx", "*.wow", "*.xm", "*.as3m",
    ];
    const FLAGS: FileFormatsCategory = FileFormatsCategory::AUDIO;
}

struct OpenMptDecoderBuilder;

impl AudioDecoderBuilder for OpenMptDecoderBuilder {
    const PRIORITY: Option<i32> = Some(1000);
    const ONLY_INSTANCE: bool = false;
    type Decoder = OpenMptDecoder;
    type Error = Error;

    fn create(&self, mut stream: Stream) -> Result<Self::Decoder, Self::Error> {
        let mut module = Module::create(&mut stream, Logger::StdErr, &[])
            .map(ModuleWrapper)
            .map_err(|()| Error::Module)?;
        module.ctl_set("render.resampler.emulate_amiga", "1");
        module.ctl_set("render.resampler.emulate_amiga_type", "a500");
        module.ctl_set("seek.sync_samples", "1");
        module.set_render_interpolation_filter_length(8);
        module.set_render_stereo_separation(200);
        if let Some(warnings) = module.metadata("warnings") {
            log::warn!("Module warnings: {}", warnings);
        }

        log::trace!("Module created");

        Ok(OpenMptDecoder(Mutex::new(DecoderInner { module })))
    }
}

struct OpenMptDecoder(Mutex<DecoderInner>);

struct DecoderInner {
    module: ModuleWrapper,
}

impl OpenMptDecoder {
    fn get(&self) -> MutexGuard<DecoderInner> {
        self.0.lock().unwrap()
    }
}

impl AudioDecoder for OpenMptDecoder {
    fn file_info(&self) -> Option<FileInfo> {
        let mut info = FileInfo::default();
        {
            let module = &mut self.get().module;

            let mut guard = info.update();
            guard
                .sample_rate(SAMPLE_RATE)
                .channels(2)
                .duration(module.duration_secs());

            if let Some(title) = module.metadata("title") {
                guard.title(title.into());
            }

            if let Some(artist) = module.metadata("artist") {
                guard.artist(artist.into());
            }

            let ty = module
                .metadata("originaltype_long")
                .or_else(|| module.metadata("type_long"));
            let tracker = module.metadata("tracker");
            let codec = match (ty, tracker) {
                (Some(ty), Some(tracker)) => Some(format!("{} / {}", ty, tracker)),
                (Some(ty), None) => Some(ty),
                (None, Some(tracker)) => Some(tracker),
                (None, None) => None,
            };
            if let Some(codec) = codec {
                guard.codec(codec.into());
            }

            if let Some(comment) = module.metadata("message") {
                guard.comment(comment.into());
            }

            if let Some(date) = module.metadata("date") {
                guard.date(date.into());
            }
        }
        Some(info)
    }

    fn stream_info(&self) -> Option<StreamInfo> {
        Some(StreamInfo {
            sample_rate: SAMPLE_RATE,
            channels: 2,
            sample_format: SampleFormat::ThirtyTwoBitFloat,
        })
    }

    fn is_seekable(&self) -> bool {
        self.size() != 0
    }

    fn is_realtime_stream(&self) -> bool {
        self.size() == 0
    }

    fn available_data(&self) -> i64 {
        let size = self.size();
        if size == 0 {
            i64::MAX
        } else {
            size - self.pos()
        }
    }

    fn size(&self) -> i64 {
        let size = self.get().module.duration();
        if size == i64::MAX {
            0
        } else {
            size
        }
    }

    fn pos(&self) -> i64 {
        self.get().module.pos()
    }

    fn set_pos(&self, pos: i64) -> bool {
        let module = &mut self.get().module;
        let pos = bytes_to_seconds(pos);
        if pos > module.duration_secs() {
            false
        } else {
            module.set_pos_secs(pos);
            true
        }
    }

    fn read(&self, buf: &mut [u8]) -> i32 {
        if buf.is_empty() {
            return 0;
        }

        self.get().module.read(buf)
    }

    fn buffering_progress(&self) -> Option<BufferingProgress> {
        BufferingProgress::new(1.0)
    }

    fn notifications<'a>(&self) -> Option<&'a AudioDecoderNotificationsWrapper> {
        None
    }
}

struct OpenMpt;

impl Plugin for OpenMpt {
    const INFO: PluginInfo = PluginInfo {
        name: "OpenMPT",
        author: "ark0f",
        short_description: "OpenMPT-based decoder written in Rust",
        full_description: Some("*.669; *.amf; *.ams; *.c67; *.dbm; *.dtm; *.far; *.gdm; *.ice; *.st26; \
                               *.imf; *.it; *.itp; *.j2b; *.m15; *.stk; *.mdl; *.med; *.mo3; *.mod; \
                               *.mptm; *.mt2; *.mtm; *.okt; *.oxm; *.plm; *.psm; *.pt36; *.ptm; *.s3m; \
                               *.sfx; *.sfx2; *.mms; *.stm; *.stp; *.ult; *.umx; *.wow; *.xm; *.as3m"),
        category: || PluginCategory::ADDONS | PluginCategory::DECODERS,
    };

    type Error = Error;

    fn new() -> Result<Self, Error> {
        pretty_env_logger::formatted_builder()
            .filter_level(LevelFilter::Trace)
            .write_style(WriteStyle::Always)
            .init();

        CORE.get()
            .register_extension(FileFormatWrapper(OpenMptFormats));
        CORE.get()
            .register_extension(AudioDecoderBuilderWrapper::new(OpenMptDecoderBuilder));

        log::trace!("Hi");

        Ok(OpenMpt)
    }

    fn finish(self) -> Result<(), Error> {
        Ok(())
    }
}

aimp::main!(OpenMpt);
