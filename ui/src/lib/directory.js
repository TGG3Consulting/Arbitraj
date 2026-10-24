// Файл: ui/src/lib/directory.js
// Что делает: подписи справочника пулов от индексатора (решение 58): итог адаптера по пулу (взят, не проверен,
//   не поддержан, DEX не подключена, проверка не прошла) и короткий вид ликвидности по данным индексатора.
// Кто вызывает: components/PoolDirectory.svelte, components/CoinDirectory.svelte.
// От чего зависит: ничего.
// Настройки: нет.

/**
 * Что делает: подпись итога пула. Принимает: status из PoolDirectory (строка или {NotConnected|Rejected: причина}).
 * Возвращает: {text, cls} — cls: ok | warn | err | muted для цвета.
 */
export function dirStatus(status) {
  if (status === 'Taken') return { text: 'взят, слушается со своего узла', cls: 'ok' };
  if (status === 'Unchecked') return { text: 'не проверен на своём узле', cls: 'muted' };
  if (status?.Unsupported) return { text: status.Unsupported, cls: 'muted' };
  if (status?.NotConnected) return { text: status.NotConnected, cls: 'warn' };
  if (status?.Rejected) return { text: `отклонён: ${status.Rejected}`, cls: 'err' };
  return { text: JSON.stringify(status), cls: 'muted' };
}

/** Ликвидность по индексатору коротко: «$1.39M», «$953», «—». Принимает: строку-число или null. */
export function usdShort(v) {
  if (v === null || v === undefined) return '—';
  const n = Number(v);
  if (!Number.isFinite(n)) return String(v);
  if (n >= 1e6) return `$${(n / 1e6).toFixed(2)}M`;
  if (n >= 1e3) return `$${(n / 1e3).toFixed(1)}K`;
  return `$${n.toFixed(0)}`;
}
