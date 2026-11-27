// Проверка собственного модуля на удержании комиссии второй передачи и на чистой выплате после сжигания.
import { join } from 'node:path';
import * as L from './lib.mjs';

const dir = join(L.here, 'fixtures');
const opt = { optimizer: { enabled: true, runs: 200 }, viaIR: true, evmVersion: 'cancun' };
const reserve = L.compile(L.solc08, dir, 'TaxToken.sol', 'TaxToken', opt);
const issuer = L.compile(L.solc08, dir, 'MintClubFixture.sol', 'MintClubFixture', opt);
const minted = L.compile(L.solc08, dir, 'MintClubFixture.sol', 'TestIssued', opt);
const module = L.compile(L.solc08, join(L.here, '..'), 'MintClubAdapter.sol', 'MintClubAdapter', opt);
const payer = '0x' + '91'.repeat(20);
const c = await L.chain();
await L.fund(c, payer, 10n ** 20n);
const cash = await L.deploy(c, payer, reserve, ['uint256'], [1000n]);
const bond = await L.deploy(c, payer, issuer, ['address'], [cash]);
const implementation = await L.view(c, bond, issuer.abi, 'implementation');
const token = await L.view(c, bond, issuer.abi, 'issued');
const target = await L.deploy(c, payer, module, ['address', 'address'], [bond, implementation]);
const balances = async () => Promise.all([
  L.view(c, cash, reserve.abi, 'balanceOf', [payer]), L.view(c, cash, reserve.abi, 'balanceOf', [bond]),
  L.view(c, token, minted.abi, 'balanceOf', [payer]), L.view(c, cash, reserve.abi, 'balanceOf', [target]),
  L.view(c, cash, reserve.abi, 'allowance', [target, bond]), L.view(c, token, minted.abi, 'balanceOf', [target]),
]);
const same = (a,b) => a.length === b.length && a.every((v,i) => v === b[i]);
const trade = (buy,minimum) => L.send(c,payer,target,module.abi,'execute',[token,cash,buy,100n,100n,minimum,1000n]);
await L.must(c,payer,cash,reserve.abi,'approve',[target,100n]);
let before = await balances();
// Плательщик освобождён от налога. Только перевод с модуля эмитенту теряет одну единицу.
const short = await trade(true,100n);
L.check(!short.ok && short.reason === 'bond payment' && same(before,await balances()),
        'недоплата эмитенту на втором переводе возвращает выпуск, остатки и allowance');
await L.must(c,payer,cash,reserve.abi,'setExempt',[target,true]);
const buy = await trade(true,100n);
L.check(buy.ok && same(await balances(),[900n,100n,100n,0n,0n,0n]), 'выпуск за полное обеспечение');
await L.must(c,payer,token,minted.abi,'approve',[target,100n]);
before = await balances();
const fee = await trade(false,100n);
L.check(!fee.ok && fee.reason === 'wallet' && same(before,await balances()), '100 номинальных единиц дают 99 чистых: минимум 100 откатывает погашение');
const sell = await trade(false,99n);
L.check(sell.ok && same(await balances(),[999n,0n,0n,0n,0n,0n]), 'точный минимум 99 даёт держателю 99 после сжигания 100');
