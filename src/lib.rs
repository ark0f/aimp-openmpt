use aimp::{
    decoders::{
        AudioDecoder, AudioDecoderBuilder, AudioDecoderBuilderWrapper,
        AudioDecoderNotificationsWrapper, StreamInfo,
    },
    file::{FileFormat, FileFormatWrapper, FileInfo},
    msg_box,
    stream::Stream,
    Plugin, PluginCategory, PluginInfo, CORE,
};
use iaimp::{BufferingProgress, FileFormatsCategory, SampleFormat};
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

struct MyFileFormat;

impl FileFormat for MyFileFormat {
    const DESCRIPTION: &'static str = "OpenMPT formats";
    const EXTS: &'static [&'static str] = &[
        "*.669", "*.amf", "*.ams", "*.c67", "*.dbm", "*.dtm", "*.far", "*.gdm", "*.ice", "*.st26",
        "*.imf", "*.it", "*.itp", "*.j2b", "*.m15", "*.stk", "*.mdl", "*.med", "*.mo3", "*.mod",
        "*.mptm", "*.mt2", "*.mtm", "*.okt", "*.oxm", "*.plm", "*.psm", "*.pt36", "*.ptm", "*.s3m",
        "*.sfx", "*.sfx2", "*.mms", "*.stm", "*.stp", "*.ult", "*.umx", "*.wow", "*.xm", "*.midi",
        "*.wav",
    ];
    const FLAGS: FileFormatsCategory = FileFormatsCategory::AUDIO;
}

const SAMPLE_RATE: i32 = 48000;

fn seconds_to_bytes(seconds: c_double) -> i64 {
    (seconds * SAMPLE_RATE as f64 * 2.0 * (32.0 / 8.0)) as i64
}

fn bytes_to_seconds(bytes: i64) -> c_double {
    bytes as f64 / (SAMPLE_RATE as f64 * 2.0 * (32.0 / 8.0))
}

struct MyAudioDecoderBuilder;

impl AudioDecoderBuilder for MyAudioDecoderBuilder {
    const PRIORITY: Option<i32> = Some(1000);
    const ONLY_INSTANCE: bool = false;
    type Decoder = MyAudioDecoder;
    type Error = Error;

    fn create(&self, mut stream: Stream) -> Result<Self::Decoder, Self::Error> {
        let mut module =
            Module::create(&mut stream, Logger::StdErr, &[]).map_err(|()| Error::Module)?;
        module.ctl_set("render.resampler.emulate_amiga", "1");
        module.ctl_set("render.resampler.emulate_amiga_type", "a500");
        module.ctl_set("seek.sync_samples", "1");
        module.set_render_interpolation_filter_length(8);
        module.set_render_stereo_separation(200);
        Ok(MyAudioDecoder(Mutex::new(MyAudioDecoderInner { module })))
    }
}

struct MyAudioDecoder(Mutex<MyAudioDecoderInner>);

struct MyAudioDecoderInner {
    module: Module,
}

impl MyAudioDecoder {
    fn get(&self) -> MutexGuard<MyAudioDecoderInner> {
        self.0.lock().unwrap()
    }
}

impl AudioDecoder for MyAudioDecoder {
    fn file_info(&self) -> Option<FileInfo> {
        let mut info = FileInfo::default();
        info.update()
            .sample_rate(SAMPLE_RATE)
            .channels(2)
            .duration(self.get().module.get_duration_seconds());
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
        let size = seconds_to_bytes(self.get().module.get_duration_seconds());
        if size == i64::MAX {
            0
        } else {
            size
        }
    }

    fn pos(&self) -> i64 {
        seconds_to_bytes(self.get().module.get_position_seconds())
    }

    fn set_pos(&self, pos: i64) -> bool {
        let module = &mut self.get().module;
        let secs = bytes_to_seconds(pos);
        if secs > module.get_duration_seconds() {
            false
        } else {
            module.set_position_seconds(secs);
            true
        }
    }

    fn read(&self, buf: &mut [u8]) -> i32 {
        if buf.is_empty() {
            return 0;
        }

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
            .get()
            .module
            .read_interleaved_float_stereo(SAMPLE_RATE, &mut stereo);

        mem::forget(stereo);

        (read * mem::size_of::<f32>() * 2) as i32
    }

    fn buffering_progress(&self) -> Option<BufferingProgress> {
        BufferingProgress::new(1.0)
    }

    fn notifications<'a>(&self) -> Option<&'a AudioDecoderNotificationsWrapper> {
        None
    }
}

struct MyPlugin;

impl Plugin for MyPlugin {
    const INFO: PluginInfo = PluginInfo {
        name: "OpenMPT",
        author: "ark0f",
        short_description: "Decoder plugin base on OpenMPT written in Rust",
        full_description: Some("*.669; *.amf; *.ams; *.c67; *.dbm; *.dtm; *.far; *.gdm; *.ice; *.st26; \
                               *.imf; *.it; *.itp; *.j2b; *.m15; *.stk; *.mdl; *.med; *.mo3; *.mod; \
                               *.mptm; *.mt2; *.mtm; *.okt; *.oxm; *.plm; *.psm; *.pt36; *.ptm; *.s3m; \
                               *.sfx; *.sfx2; *.mms; *.stm; *.stp; *.ult; *.umx; *.wow; *.xm; *.midi; \
                               *.wav"),
        category: || PluginCategory::ADDONS | PluginCategory::DECODERS,
    };

    type Error = Error;

    fn new() -> Result<Self, Error> {
        pretty_env_logger::formatted_builder()
            .filter_level(LevelFilter::Trace)
            .write_style(WriteStyle::Always)
            .init();

        CORE.get()
            .register_extension(FileFormatWrapper(MyFileFormat));
        CORE.get()
            .register_extension(AudioDecoderBuilderWrapper::new(MyAudioDecoderBuilder));

        log::trace!("Hi");

        Ok(MyPlugin)
    }

    fn finish(self) -> Result<(), Error> {
        Ok(())
    }
}

aimp::main!(MyPlugin);
