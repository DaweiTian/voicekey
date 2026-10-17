import type { ASRConfig, CloudDialect } from './types'

/** A cloud ASR provider preset: backfills endpoint + dialect + an example model. */
export interface CloudAsrPreset {
  id: string
  /** i18n key for the dropdown label. */
  labelKey: string
  /** Endpoint to backfill; empty means "leave the user's value untouched". */
  endpoint: string
  /** Dialect to backfill; empty means "leave the user's choice untouched". */
  dialect: CloudDialect | ''
  /** Example model to backfill; empty means "leave it for the user to fill". */
  model: string
  /** Placeholder shown in the model field when it is empty. */
  modelPlaceholder: string
}

/**
 * Provider presets. Selecting one backfills `endpoint` + `dialect` + example
 * `model`; the API key is never touched, and `custom` backfills nothing so the
 * user's hand-written values survive. Endpoints/dialects per the 0.5.12 vendor
 * API research (see README).
 */
export const CLOUD_ASR_PRESETS: readonly CloudAsrPreset[] = [
  {
    id: 'mimo',
    labelKey: 'settings.cloudPresetMimo',
    endpoint: 'https://api.xiaomimimo.com/v1',
    dialect: 'chat_asr_data_url',
    model: 'mimo-v2.5-asr',
    modelPlaceholder: 'mimo-v2.5-asr',
  },
  {
    id: 'qwen',
    labelKey: 'settings.cloudPresetQwen',
    endpoint: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    dialect: 'chat_asr_data_url',
    model: 'qwen3-asr-flash',
    modelPlaceholder: 'qwen3-asr-flash',
  },
  {
    id: 'doubao_ark',
    labelKey: 'settings.cloudPresetDoubaoArk',
    endpoint: 'https://ark.cn-beijing.volces.com/api/v3',
    dialect: 'chat_openai_audio',
    model: '',
    modelPlaceholder: 'doubao-seed-… / ep-…',
  },
  {
    id: 'openai',
    labelKey: 'settings.cloudPresetOpenai',
    endpoint: 'https://api.openai.com/v1',
    dialect: 'chat_openai_audio',
    model: '',
    modelPlaceholder: 'gpt-audio-1.5',
  },
  {
    id: 'custom',
    labelKey: 'settings.cloudPresetCustom',
    endpoint: '',
    dialect: '',
    model: '',
    modelPlaceholder: 'model-id',
  },
]

/** Dialect choices shown only for the `custom` preset (label + tooltip keys). */
export const CLOUD_ASR_DIALECTS: readonly {
  id: CloudDialect
  labelKey: string
  helpKey: string
}[] = [
  {
    id: 'chat_asr_data_url',
    labelKey: 'settings.cloudDialectDataUrl',
    helpKey: 'settings.cloudDialectDataUrlHelp',
  },
  {
    id: 'chat_openai_audio',
    labelKey: 'settings.cloudDialectOpenAiAudio',
    helpKey: 'settings.cloudDialectOpenAiAudioHelp',
  },
  {
    id: 'openai_transcribe',
    labelKey: 'settings.cloudDialectTranscribe',
    helpKey: 'settings.cloudDialectTranscribeHelp',
  },
]

export const DEFAULT_CLOUD_PRESET = 'mimo'
export const DEFAULT_CLOUD_DIALECT: CloudDialect = 'chat_asr_data_url'

/** Resolve a preset id, falling back to the first (MiMo) preset. */
export function getCloudAsrPreset(id: string | undefined): CloudAsrPreset {
  return CLOUD_ASR_PRESETS.find((preset) => preset.id === id) ?? CLOUD_ASR_PRESETS[0]
}

/**
 * Apply a preset to an ASR config: backfill endpoint + dialect + example model
 * and record the selected preset id. `custom` leaves endpoint/dialect/model
 * untouched; the API key is never touched by any preset.
 */
export function applyCloudAsrPreset(asr: ASRConfig, presetId: string): ASRConfig {
  const preset = getCloudAsrPreset(presetId)
  if (preset.id === 'custom') {
    return { ...asr, cloudPreset: 'custom' }
  }
  return {
    ...asr,
    cloudPreset: preset.id,
    cloudEndpoint: preset.endpoint,
    cloudDialect: preset.dialect as CloudDialect,
    cloudModel: preset.model,
  }
}
