import { describe, expect, it } from 'vitest'
import { storageKindLabelKey, storageSwitchLabelKey } from './storage'

describe('storage label keys', () => {
  it('names the current root kind', () => {
    expect(storageKindLabelKey('appData')).toBe('settings.storage.kindAppData')
    expect(storageKindLabelKey('portable')).toBe('settings.storage.kindPortable')
  })

  it('names the opposite root as the switch target', () => {
    // The button always offers the *other* root.
    expect(storageSwitchLabelKey('portable')).toBe('settings.storage.switchToPortable')
    expect(storageSwitchLabelKey('appData')).toBe('settings.storage.switchToAppData')
  })

  it('covers both kinds with distinct keys', () => {
    const kinds = ['appData', 'portable'] as const
    const kindKeys = kinds.map(storageKindLabelKey)
    const switchKeys = kinds.map(storageSwitchLabelKey)
    expect(new Set(kindKeys).size).toBe(2)
    expect(new Set(switchKeys).size).toBe(2)
  })
})
