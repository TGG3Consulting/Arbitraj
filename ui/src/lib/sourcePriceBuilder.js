// У Lambda список приходит из настроек конкретного DEX. bopAMM использует два канала своей формы.
export function priceBuilder(source, allowed, previous) {
  const required = allowed !== null || source === 'bopamm';
  const choices = allowed ?? (source === 'bopamm' ? ['buildernet', 'titan'] : []);
  return {required, choices, selected: choices.includes(previous) ? previous : choices[0]};
}
