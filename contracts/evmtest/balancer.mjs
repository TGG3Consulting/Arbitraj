// Файл: contracts/evmtest/balancer.mjs
// Что делает: решение 289 — шаг BALANCER (вид 17) ArbExecutor на EVM ethereumjs (Cancun) с хранилищем-фикстурой
//   fixtures/BalancerVaultFixture.sol (подпись swap и строки обмена Vault: вызывающий = sender, transferFrom входа,
//   выход recipient, limit):
//   B1) v2: execute USDT → COIN шагом 17 (пул шага — хранилище, data = poolId) в обе стороны: владелец получил выход
//       хранилища, на исполнителе пусто, разрешение исполнителя хранилищу после шага — 0; minOut + 1 — откат;
//   B2) чужой poolId — откат (хранилище не знает пул); data не 32 байта — откат;
//   B3) исполнитель v1 — шаг 17 откатывается «kind».
//   Проба на настоящем хранилище ACSI на цепи — tests/data/balancer_acsi_2026-11-15/exec_sim.json.
// Кто вызывает: contracts/evmtest/run.mjs.
// От чего зависит: lib.mjs, world.mjs, fixtures/BalancerVaultFixture.sol.
// Настройки: нет.

import { join } from 'node:path';
import * as L from './lib.mjs';
import * as W from './world.mjs';

const { check, must } = L;
const { E18, OWNER, LP, ERC20, v1, v2, world, exe, bal } = W;
const BV = L.compile(L.solc08, join(L.here, 'fixtures'), 'BalancerVaultFixture.sol', 'BalancerVaultFixture', W.opt);
const ID = '0x' + '7e'.repeat(20) + '0001' + '00'.repeat(9) + '01';
const step = (vault, tin, tout, data = ID) => [17, vault, tin, tout, 10000, data];

/// Хранилище-фикстура с 10 000 USDT и 10 000 COIN от LP. Возвращает адрес.
async function vaultOf(c, usdt, coin) {
  const v = await L.deploy(c, LP, BV, ['bytes32'], [ID]);
  await must(c, LP, usdt, ERC20.abi, 'transfer', [v, 10_000n * E18]);
  await must(c, LP, coin, ERC20.abi, 'transfer', [v, 10_000n * E18]);
  return v;
}

/// Сценарии B1–B3. Ошибок нет: провал — check бросает.
export async function balancer() {
  {
    const { c, usdt, coin, ex } = await world(v2);
    const v = await vaultOf(c, usdt, coin);
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    await must(c, OWNER, coin, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    const amt = 100n * E18;
    const exp = amt * 99n / 100n;
    const bad = await L.call(c, OWNER, ex, exe(usdt, amt, coin, exp + 1n, [step(v, usdt, coin)]));
    check(!bad.ok && bad.reason === 'minOut', 'BALANCER: minOut + 1 — откат «minOut»');
    for (const [tin, tout, name] of [[usdt, coin, 'USDT → COIN'], [coin, usdt, 'COIN → USDT']]) {
      const before = await bal(c, tout, OWNER);
      const r = await L.call(c, OWNER, ex, exe(tin, amt, tout, exp, [step(v, tin, tout)]));
      check(r.ok, `BALANCER: execute ${name} шагом 17 прошёл (газ ${r.gas})`);
      check((await bal(c, tout, OWNER)) - before === exp, `BALANCER: ${name} — владелец получил выход хранилища`);
      check((await bal(c, tin, ex)) === 0n && (await bal(c, tout, ex)) === 0n, `BALANCER: ${name} — на исполнителе ничего не осталось`);
      const left = await L.view(c, tin, ERC20.abi, 'allowance', [ex, v]);
      check(BigInt(left) === 0n, `BALANCER: ${name} — разрешение исполнителя хранилищу после шага 0`);
    }
    const other = '0x' + '11'.repeat(20) + '0001' + '00'.repeat(9) + '01';
    const w = await L.call(c, OWNER, ex, exe(usdt, amt, coin, 1n, [step(v, usdt, coin, other)]));
    check(!w.ok, `BALANCER: чужой poolId — откат (${w.reason})`);
    const s = await L.call(c, OWNER, ex, exe(usdt, amt, coin, 1n, [step(v, usdt, coin, '0x01')]));
    check(!s.ok, 'BALANCER: data не 32 байта — откат');
  }
  {
    const { c, usdt, coin, ex } = await world(v1);
    const v = await vaultOf(c, usdt, coin);
    await must(c, OWNER, usdt, ERC20.abi, 'approve', [ex, 10n ** 30n]);
    const r = await L.call(c, OWNER, ex, exe(usdt, E18, coin, 1n, [step(v, usdt, coin)]));
    check(!r.ok && r.reason === 'kind', 'исполнитель v1: шаг BALANCER (17) — откат «kind» (вида нет в v1)');
  }
}
