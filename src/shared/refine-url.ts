const CHAT_COMPLETIONS_PATH = '/chat/completions'

export function normalizeRefineBaseUrl(endpoint: string): string {
  const trimmed = endpoint.trim()
  if (!trimmed) {
    return ''
  }

  const withoutTrailingSlash = trimmed.replace(/\/+$/, '')
  if (!withoutTrailingSlash.endsWith(CHAT_COMPLETIONS_PATH)) {
    return withoutTrailingSlash
  }

  return withoutTrailingSlash.slice(0, -CHAT_COMPLETIONS_PATH.length).replace(/\/+$/, '')
}

export function buildRefineChatEndpoint(baseUrl: string): string {
  const normalizedBaseUrl = normalizeRefineBaseUrl(baseUrl)
  if (!normalizedBaseUrl) {
    return ''
  }

  return `${normalizedBaseUrl}${CHAT_COMPLETIONS_PATH}`
}
