import assert from 'node:assert/strict';
import { test } from 'node:test';
import { explainSize } from '../src/lib/sizeExplanation.js';

// Сохранённая строка plan.size.why сделки 20261118213857378SIREN, MEXC → BSC.
const siren = 'оптимум 2936.62 SIREN: прибыль 0.1673; на бюджете 96.3215 (46.11 SIREN) — -0.076; допустимо 40.66–46.11 SIREN, посчитано объёмов 68, пиков 1';

test('SIREN shows the chosen quantity and the first search boundary separately', () => {
  assert.equal(explainSize(siren), 'Подбор выбрал 2936.62 SIREN, прибыль 0.1673 USDT при бюджете 96.3215 USDT. Граница участка поиска — 46.11 SIREN, прибыль -0.076 USDT. Нижняя найденная точка 40.66 SIREN. Общий максимум объёма не установлен. Посчитано объёмов 68, пиков 1');
});

test('historical AKE rejection retains both boundaries and the archived bisection reason without a size object', () => {
  // docs/audits/2026-11-23-recent-episodes.json, episodes[4].close_reason.
  const reason = 'прибыль -0.0245 USDT ниже порога 0.1 (оптимум 392.75 AKE: прибыль -0.0245; на бюджете 1000 (380866.25 AKE) — -20.504; допустимо 392.75–380866.25 AKE, посчитано объёмов 143, пиков 2; граница бюджета не уточнена до шага (мало size.bisect_steps))';
  const shown = explainSize(reason);
  assert.ok(shown.startsWith('прибыль -0.0245 USDT ниже порога 0.1 (Подбор выбрал 392.75 AKE'));
  assert.ok(shown.includes('Граница участка поиска — 380866.25 AKE, прибыль -20.504 USDT. Нижняя найденная точка 392.75 AKE.'));
  assert.ok(shown.endsWith('Посчитано объёмов 143, пиков 2; граница участка не уточнена до шага (в архиве указано: мало size.bisect_steps))'));
});

test('keeps simulation limits, threshold retries and the final resize visible', () => {
  const base = 'оптимум 402 ACE: прибыль 19.2122; это и есть наибольший объём в пределах бюджета 7.3989; допустимо 4–402 ACE, посчитано объёмов 55, пиков 1';
  const resize = '; по финальной котировке объём ужат (бюджет или глубина исполнителя): 203 ACE';
  const prefix = 'объём ограничен симуляцией продажи до покупки (решение 380); подбор на проверяемом объёме: ';
  const suffix = '; поиск остановлен пределом size.max_evals — лучший из посчитанных; по финальной котировке на 203 ACE прибыль в % ниже порога 2 — взят ближайший меньший проходящий оба порога объём 193 ACE';
  const text = explainSize(prefix + base + resize + suffix);
  assert.ok(text.startsWith(prefix + 'Подбор выбрал 402 ACE'));
  assert.ok(text.includes('Граница участка поиска — 402 ACE, прибыль 19.2122 USDT'));
  assert.ok(text.endsWith(resize + suffix));
  assert.ok(explainSize(`ниже порога (${siren})`).startsWith('ниже порога (Подбор выбрал'));
});

test('one-point, cycle, new and unrecognized archive formats pass through verbatim', () => {
  for (const text of [undefined, '', 'объём кандидата 5 CAKE пересчитан на свежих данных: прибыль 1; повторный подбор объёма не выполнялся (precheck.fast_recalc)',
    'наибольшая чистая прибыль цикла 1 USDT на входе 10 (бюджет 20 USDT)',
    explainSize(siren), 'иной формат; допустимо 1–2 CAKE, посчитано объёмов 3']) {
    assert.equal(explainSize(text), text);
  }
});
