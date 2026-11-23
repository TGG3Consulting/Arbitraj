// Старые полные подборы включали диапазон «допустимо» между причиной выбора и
// пояснениями исполнителя. Заменяем только узнаваемый фрагмент из profit::why.
const oldRange = /оптимум (?<chosen>[\d.]+) (?<ticker>[^;]+?): прибыль (?<profit>-?[\d.]+); (?:на бюджете (?<budget>[\d.]+) \([\d.]+ \k<ticker>\) — (?<edgeProfit>-?[\d.]+)|это и есть наибольший объём в пределах бюджета (?<fullBudget>[\d.]+)); допустимо (?<lower>[\d.]+)–(?<edge>[\d.]+) \k<ticker>, посчитано объёмов (?<evals>\d+), пиков (?<peaks>\d+)(?<inexact>; граница бюджета не уточнена до шага \(мало size\.bisect_steps\))?/u;

export function explainSize(text) {
  if (!text?.includes('; допустимо ')) return text;
  const match = oldRange.exec(text);
  if (!match) return text;
  const { chosen, ticker, profit, budget, fullBudget, edge, edgeProfit, lower, evals, peaks, inexact } = match.groups;
  const explanation =
    `Подбор выбрал ${chosen} ${ticker}, прибыль ${profit} USDT при бюджете ${budget ?? fullBudget} USDT. ` +
    `Граница участка поиска — ${edge} ${ticker}, прибыль ${edgeProfit ?? profit} USDT. ` +
    `Нижняя найденная точка ${lower} ${ticker}. Общий максимум объёма не установлен. ` +
    `Посчитано объёмов ${evals}, пиков ${peaks}` +
    (inexact ? '; граница участка не уточнена до шага (в архиве указано: мало size.bisect_steps)' : '');
  return text.slice(0, match.index) + explanation + text.slice(match.index + match[0].length);
}
