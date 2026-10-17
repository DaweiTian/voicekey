import { describe, expect, it } from 'vitest'
import {
  CLOUD_ASR_DIALECTS,
  CLOUD_ASR_PRESETS,
  DEFAULT_CLOUD_DIALECT,
  DEFAULT_CLOUD_PRESET,
  applyCloudAsrPreset,
  getCloudAsrPreset,
} from './cloud-asr'
import type { ASRConfig } from './types'

const baseAsr: ASRConfig = {
  lowVolumeMode: true,
  microphoneDeviceId: '',
  microphoneDeviceLabel: '',
  cloudEndpoint: '',
  cloudApiKey: 'secret-key',
  cloudModel: '',
  cloudPreset: 'mimo',
  cloudDialect: 'chat_asr_data_url',
}

describe('cloud ASR preset table', () => {
  it('exposes the four providers plus custom, with MiMo first as the default', () => {
    expect(CLOUD_ASR_PRESETS.map((preset) => preset.id)).toEqual([
      'mimo',
      'qwen',
      'doubao_ark',
      'openai',
      'custom',
    ])
    expect(CLOUD_ASR_PRESETS[0].id).toBe(DEFAULT_CLOUD_PRESET)
  })

  it('matches the researched endpoint and dialect per provider', () => {
    expect(getCloudAsrPreset('mimo').endpoint).toBe('https://api.xiaomimimo.com/v1')
    expect(getCloudAsrPreset('mimo').dialect).toBe('chat_asr_data_url')
    expect(getCloudAsrPreset('qwen').endpoint).toBe(
      'https://dashscope.aliyuncs.com/compatible-mode/v1',
    )
    expect(getCloudAsrPreset('qwen').dialect).toBe('chat_asr_data_url')
    expect(getCloudAsrPreset('doubao_ark').endpoint).toBe(
      'https://ark.cn-beijing.volces.com/api/v3',
    )
    expect(getCloudAsrPreset('doubao_ark').dialect).toBe('chat_openai_audio')
    expect(getCloudAsrPreset('openai').endpoint).toBe('https://api.openai.com/v1')
    expect(getCloudAsrPreset('openai').dialect).toBe('chat_openai_audio')
  })

  it('falls back to the MiMo preset for an unknown id', () => {
    expect(getCloudAsrPreset('nope').id).toBe('mimo')
    expect(getCloudAsrPreset(undefined).id).toBe('mimo')
  })

  it('lists the three dialects with distinct labels and help keys', () => {
    expect(CLOUD_ASR_DIALECTS.map((dialect) => dialect.id)).toEqual([
      'chat_asr_data_url',
      'chat_openai_audio',
      'openai_transcribe',
    ])
    expect(new Set(CLOUD_ASR_DIALECTS.map((dialect) => dialect.helpKey)).size).toBe(3)
    expect(DEFAULT_CLOUD_DIALECT).toBe('chat_asr_data_url')
  })
})

describe('applyCloudAsrPreset', () => {
  it('backfills endpoint, dialect and example model without touching the API key', () => {
    const next = applyCloudAsrPreset(baseAsr, 'qwen')

    expect(next.cloudPreset).toBe('qwen')
    expect(next.cloudEndpoint).toBe('https://dashscope.aliyuncs.com/compatible-mode/v1')
    expect(next.cloudDialect).toBe('chat_asr_data_url')
    expect(next.cloudModel).toBe('qwen3-asr-flash')
    expect(next.cloudApiKey).toBe('secret-key')
  })

  it('leaves endpoint/dialect/model untouched for the custom preset', () => {
    const edited: ASRConfig = {
      ...baseAsr,
      cloudEndpoint: 'https://my-gateway.example.com/v1',
      cloudDialect: 'openai_transcribe',
      cloudModel: 'my-model',
    }

    const next = applyCloudAsrPreset(edited, 'custom')

    expect(next.cloudPreset).toBe('custom')
    expect(next.cloudEndpoint).toBe('https://my-gateway.example.com/v1')
    expect(next.cloudDialect).toBe('openai_transcribe')
    expect(next.cloudModel).toBe('my-model')
    expect(next.cloudApiKey).toBe('secret-key')
  })

  it('is a no-op on the key for every preset', () => {
    for (const preset of CLOUD_ASR_PRESETS) {
      expect(applyCloudAsrPreset(baseAsr, preset.id).cloudApiKey).toBe('secret-key')
    }
  })
})
