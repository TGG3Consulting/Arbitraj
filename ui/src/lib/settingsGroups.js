// Файл: ui/src/lib/settingsGroups.js
// Что делает: категории экрана «Настройки» — это поле group схемы (GET /api/settings/schema), своих категорий
//   в интерфейсе нет. По каждой: сколько настроек и сколько не задано нигде; поиск по имени и описанию
//   во всех категориях с группировкой по категориям; фильтр «только незаданные» в одной категории или во всех.
//   Порядок категорий — как в схеме (по первой настройке категории).
// Кто вызывает: pages/Settings.svelte, components/SettingsGroupList.svelte, components/NavMenu.svelte.
// От чего зависит: ничего (чистые функции над схемой и значениями из lib/settings.svelte.js).
// Настройки: нет.

/**
 * Задана ли настройка хоть для одной области («не задано» = не задана нигде, как раньше на экране).
 * Принимает: описание настройки, store.values (ключи «имя|вид|id»). Возвращает: bool.
 */
export const isSet = (def, values) => Object.keys(values).some((k) => k.startsWith(`${def.name}|`));

/**
 * Категории со счётчиками. Принимает: схему (или null) и store.values.
 * Возвращает: [{group, total, unset}] в порядке схемы; нет схемы — [].
 */
export function groupStats(schema, values) {
  if (!schema) return [];
  const out = new Map();
  for (const d of schema.settings) {
    if (!out.has(d.group)) out.set(d.group, { group: d.group, total: 0, unset: 0 });
    const g = out.get(d.group);
    g.total += 1;
    // Необязательная (d.optional) не задана — это не пробел в настройке, в «не задано» не считается.
    if (!d.optional && !isSet(d, values)) g.unset += 1;
  }
  return [...out.values()];
}

/**
 * Настройки под фильтры, сгруппированные по категориям.
 * Принимает: схему, store.values и f = {text, onlyUnset, group}:
 *   text — поиск по имени и описанию (непустой — ищет во ВСЕХ категориях, group не учитывается);
 *   onlyUnset — только не заданные нигде; group — категория ('' — все категории).
 * Возвращает: [[категория, [описания]]] в порядке схемы; пустые категории не попадают.
 */
export function filterSettings(schema, values, f) {
  if (!schema) return [];
  const q = f.text.trim().toLowerCase();
  const out = new Map();
  for (const d of schema.settings) {
    if (q) {
      if (!d.name.toLowerCase().includes(q) && !d.description.toLowerCase().includes(q)) continue;
    } else if (f.group && d.group !== f.group) continue;
    if (f.onlyUnset && (d.optional || isSet(d, values))) continue;
    if (!out.has(d.group)) out.set(d.group, []);
    out.get(d.group).push(d);
  }
  return [...out];
}

/** Подпись категории со счётчиками: «Сеть BSC — 12, не задано 2». Принимает: элемент groupStats. */
export const groupLabel = (g) => `${g.group} — ${g.total}${g.unset ? `, не задано ${g.unset}` : ''}`;
