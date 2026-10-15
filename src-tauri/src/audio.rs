use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AudioChunkPayload {
    pub session_id: String,
    pub chunk_index: u32,
    pub is_final: bool,
    pub mime_type: String,
    pub buffer: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StreamingAudioFramePayload {
    pub session_id: String,
    pub sequence: u32,
    pub sample_rate: u32,
    pub buffer: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StreamingAudioEndPayload {
    pub session_id: String,
    pub sequence: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RecordingStartPayload {
    pub session_id: String,
    pub mode: String,
    #[serde(rename = "asrMode")]
    pub asr_mode: String,
    pub microphone_device_id: Option<String>,
    pub low_volume_mode: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub enum SessionStatus {
    #[default]
    Idle,
    Recording,
    Processing,
    Completed,
    Error,
}

#[derive(Debug, Default)]
pub struct AudioSession {
    pub id: String,
    pub status: SessionStatus,
    pub transcript: String,
    pub error: Option<String>,
    pub chunks: BTreeMap<u32, Vec<u8>>,
    pub pcm: Vec<f32>,
    pub sample_rate: u32,
    pub streaming_final: bool,
    pub final_chunk_seen: bool,
    pub started_at: u64,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl AudioSession {
    pub fn start(&mut self, streaming: bool) -> RecordingStartPayload {
        let id = format!("session-{}", now_ms());
        self.id = id.clone();
        self.status = SessionStatus::Recording;
        self.transcript.clear();
        self.error = None;
        self.chunks.clear();
        self.pcm.clear();
        self.sample_rate = 16_000;
        self.streaming_final = false;
        self.final_chunk_seen = false;
        self.started_at = now_ms();
        let mode = if streaming { "streaming" } else { "classic" };
        RecordingStartPayload {
            session_id: id,
            mode: mode.into(),
            asr_mode: mode.into(),
            microphone_device_id: None,
            low_volume_mode: true,
        }
    }

    pub fn push_chunk(&mut self, payload: AudioChunkPayload) -> bool {
        if self.status != SessionStatus::Recording && self.status != SessionStatus::Processing {
            return false;
        }
        if payload.session_id != self.id {
            return false;
        }
        if payload.session_id.is_empty()
            || payload.session_id.len() > 128
            || payload
                .session_id
                .chars()
                .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        {
            return false;
        }
        self.chunks.insert(payload.chunk_index, payload.buffer);
        if payload.is_final {
            self.final_chunk_seen = true;
        }
        payload.is_final
    }

    pub fn push_streaming_frame(&mut self, payload: StreamingAudioFramePayload) -> bool {
        if (self.status != SessionStatus::Recording && self.status != SessionStatus::Processing)
            || payload.session_id != self.id
        {
            return false;
        }
        // Float32 LE samples
        let mut samples = Vec::with_capacity(payload.buffer.len() / 4);
        for chunk in payload.buffer.chunks_exact(4) {
            let mut bytes = [0u8; 4];
            bytes.copy_from_slice(chunk);
            samples.push(f32::from_le_bytes(bytes));
        }
        self.pcm.extend_from_slice(&samples);
        if payload.sample_rate > 0 {
            self.sample_rate = payload.sample_rate;
        }
        true
    }

    pub fn finish_streaming(&mut self, payload: StreamingAudioEndPayload) -> bool {
        if payload.session_id != self.id {
            return false;
        }
        self.streaming_final = true;
        self.final_chunk_seen = true;
        true
    }

    pub fn take_pcm_wav(&mut self) -> Vec<u8> {
        let samples = std::mem::take(&mut self.pcm);
        crate::asr::encode_wav_mono_f32(&samples, self.sample_rate)
    }

    pub fn has_audio(&self) -> bool {
        !self.pcm.is_empty() || self.chunks.values().any(|c| !c.is_empty())
    }

    pub fn complete(&mut self, transcript: String) {
        self.transcript = transcript;
        self.status = SessionStatus::Completed;
    }

    pub fn fail(&mut self, error: String) {
        self.error = Some(error);
        self.status = SessionStatus::Error;
    }

    pub fn cancel(&mut self) {
        self.chunks.clear();
        self.pcm.clear();
        self.status = SessionStatus::Idle;
        self.error = Some("cancelled".into());
    }

    pub fn status_name(&self) -> String {
        match self.status {
            SessionStatus::Idle => "idle",
            SessionStatus::Recording => "recording",
            SessionStatus::Processing => "processing",
            SessionStatus::Completed => "completed",
            SessionStatus::Error => "error",
        }
        .into()
    }
}
