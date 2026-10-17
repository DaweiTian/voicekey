import type { StorageRootKind } from './types'

/**
 * i18n key for the caption under the storage path that names the *current*
 * root kind.
 */
export function storageKindLabelKey(kind: StorageRootKind): string {
  return kind === 'portable' ? 'settings.storage.kindPortable' : 'settings.storage.kindAppData'
}

/**
 * i18n key for the switch button label. It names the **target** root the click
 * would migrate to — the opposite of the current one.
 */
export function storageSwitchLabelKey(target: StorageRootKind): string {
  return target === 'portable'
    ? 'settings.storage.switchToPortable'
    : 'settings.storage.switchToAppData'
}
