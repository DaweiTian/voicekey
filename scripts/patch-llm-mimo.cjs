const fs = require('fs')
const f = 'src/shared/llm-config.ts'
let s = fs.readFileSync(f, 'utf8')

s = s.replace(
  "  TokenDanceConfig,\n} from './types'",
  "  TokenDanceConfig,\n  MimoConfig,\n} from './types'",
)

s = s.replace(
  'const DEFAULT_CUSTOM_CONFIG: CustomCompatibleLLMConfig = {',
  `const DEFAULT_MIMO_CONFIG: MimoConfig = {
  apiKey: LLM_REFINE.API_KEY,
  model: 'mimo-v2.5',
  endpoint: 'https://api.xiaomimimo.com/v1',
}

const DEFAULT_CUSTOM_CONFIG: CustomCompatibleLLMConfig = {`,
)

s = s.replace(
  '  tokendance: DEFAULT_TOKENDANCE_CONFIG,\n  custom: DEFAULT_CUSTOM_CONFIG,\n}',
  '  tokendance: DEFAULT_TOKENDANCE_CONFIG,\n  mimo: DEFAULT_MIMO_CONFIG,\n  custom: DEFAULT_CUSTOM_CONFIG,\n}',
)

s = s.replace(
  "    value === 'tokendance' ||\n    value === 'custom-compatible'",
  "    value === 'tokendance' ||\n    value === 'mimo' ||\n    value === 'custom-compatible'",
)

s = s.replace(
  "  if (lowerEndpoint.includes('tokendance.space')) {\n    return 'tokendance'\n  }",
  "  if (lowerEndpoint.includes('tokendance.space')) {\n    return 'tokendance'\n  }\n  if (lowerEndpoint.includes('xiaomimimo.com') || lowerEndpoint.includes('mimo.mi.com')) {\n    return 'mimo'\n  }",
)

s = s.replace(
  `  if (config.provider === 'tokendance') {
    return {
      provider: 'tokendance',
      endpoint: config.endpoint,
      model: config.tokendance.model,
      apiKey: config.tokendance.apiKey,
    }
  }`,
  `  if (config.provider === 'tokendance') {
    return {
      provider: 'tokendance',
      endpoint: config.endpoint,
      model: config.tokendance.model,
      apiKey: config.tokendance.apiKey,
    }
  }
  if (config.provider === 'mimo') {
    return {
      provider: 'mimo',
      endpoint: config.mimo.endpoint || 'https://api.xiaomimimo.com/v1',
      model: config.mimo.model || 'mimo-v2.5',
      apiKey: config.mimo.apiKey,
    }
  }`,
)

// normalizeLLMRefineConfig returned object
if (!s.includes('mimo: {')) {
  s = s.replace(
    '    tokendance,\n    custom,',
    `    tokendance,
    mimo: {
      apiKey: readString(rawConfig?.mimo?.apiKey, DEFAULT_MIMO_CONFIG.apiKey),
      model: readString(rawConfig?.mimo?.model, DEFAULT_MIMO_CONFIG.model),
      endpoint: readString(rawConfig?.mimo?.endpoint, DEFAULT_MIMO_CONFIG.endpoint),
    },
    custom,`,
  )
}

fs.writeFileSync(f, s, 'utf8')
console.log('patched', f, 'has mimo:', s.includes("value === 'mimo'"))
