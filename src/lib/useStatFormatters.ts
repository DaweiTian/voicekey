import { useMemo } from 'react'
import { useTranslation } from 'react-i18next'
import { getLocale } from '@/shared/i18n'

/** 缁熶竴鐨勬暟瀛?/ 鏃堕暱 / 宄板€兼棩鏍煎紡鍖栵紝渚涗富椤典笌缁熻缁勪欢鍏辩敤銆?*/
export function useStatFormatters() {
  const { t, i18n } = useTranslation()
  const locale = getLocale(i18n.language)
  const numberFormatter = useMemo(() => new Intl.NumberFormat(locale), [locale])

  const formatNumber = (value: number): string => numberFormatter.format(value)

  const formatDuration = (milliseconds: number): string => {
    const totalMinutes = Math.round(milliseconds / 60000)
    const hours = Math.floor(totalMinutes / 60)
    const minutes = totalMinutes % 60

    if (hours === 0) {
      return t('time.minutes', { count: minutes, formattedCount: formatNumber(minutes) })
    }
    if (minutes === 0) {
      return t('time.hours', { count: hours, formattedCount: formatNumber(hours) })
    }
    return t('time.hoursMinutes', {
      hours: t('time.hours', { count: hours, formattedCount: formatNumber(hours) }),
      minutes: t('time.minutes', { count: minutes, formattedCount: formatNumber(minutes) }),
    })
  }

  const formatPeakDay = (dateKey: string | null): string => {
    if (!dateKey) return '-'
    const date = new Date(`${dateKey}T00:00:00`)
    return date.toLocaleDateString(locale, { month: 'short', day: 'numeric' })
  }

  return { t, locale, formatNumber, formatDuration, formatPeakDay }
}
