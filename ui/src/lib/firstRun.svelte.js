// Файл: ui/src/lib/firstRun.svelte.js
// Что делает: СОВЕТУЕМЫЕ стартовые значения для ПРЕДзаполнения формы настроек. Экран «Первый запуск»
//   кладёт сюда советуемые значения (по ключу области, как в lib/settings.svelte.js), а SettingEditor
//   при открытии подставляет их в поле ввода как «не сохранено». НИЧЕГО не сохраняет — Тигран смотрит и
//   жмёт «Сохранить» обычным путём (с журналом изменений). Значение одноразовое: editor его «съедает».
// Кто вызывает: pages/FirstRun.svelte (кладёт), components/SettingEditor.svelte (берёт).
// От чего зависит: lib/settings.svelte.js (valueKey — тот же формат ключа).
// Настройки: нет.

import { valueKey } from './settings.svelte.js';

/** map: ключ области «имя|вид|id» → текст для поля ввода. */
const state = $state({ map: {} });

/**
 * Что делает: кладёт советуемые значения для предзаполнения (заменяет прежние).
 * Принимает: список {name, scope:{kind,id?}, value} — только строки с готовым значением и областью.
 * Возвращает: ничего.
 */
export function setPrefill(entries) {
  const map = {};
  for (const e of entries) map[valueKey(e.name, e.scope)] = e.value;
  state.map = map;
}

/**
 * Что делает: забирает советуемое значение для области и удаляет его (одноразово).
 * Принимает: имя настройки, область {kind, id?}.
 * Возвращает: строку для поля ввода или undefined, если совета нет.
 */
export function takePrefill(name, scope) {
  const key = valueKey(name, scope);
  const v = state.map[key];
  if (v !== undefined) delete state.map[key];
  return v;
}
