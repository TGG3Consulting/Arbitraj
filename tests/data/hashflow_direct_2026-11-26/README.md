# Прямой Hashflow RFQT в четырёх сетях

Записаны публичные транзакции, которые уже исполнились и истекли. В каждой записи `eth_getTransactionByHash` даёт адрес роутера, отправителя, value и фактическую calldata. `eth_getTransactionReceipt` даёт статус, газ и лог пула. Хеш квитанции и хеш транзакционного блока сверены с `eth_getBlockByNumber` по высоте. В `swaps.json` сохранены только логи Trade и необходимые поля. Время истечения подписанной котировки проверяется относительно времени добытого блока.

| Сеть | Chain ID | Блок | Транзакция |
|---|---:|---:|---|
| Ethereum | 1 | 26439388 | [Etherscan](https://etherscan.io/tx/0xafe6f9081a9bb73249d74c5c035de39d4696f69f20f1115aea7acda8a9bbd147) |
| BSC | 56 | 85821758 | [BscScan](https://bscscan.com/tx/0x345c676646b88894e4bde88466461ec474f56bcf4f8325453be2c25da83644d9) |
| Base | 8453 | 30560603 | [BaseScan](https://basescan.org/tx/0xc285ebd7544a1f98920fd63edaf02f7d27c0a2a0d6943fcb3571a4aea687d72c) |
| Arbitrum One | 42161 | 441395898 | [Arbiscan](https://arbiscan.io/tx/0xd8ccf4630a66a1605eeee5b590c4f0571131cee7769d568ad1fce50a461c6cce) |

Официальные [адреса роутеров](https://github.com/hashflownetwork/x-protocol/blob/main/evm/deployed-contracts/IHashflowRouter.json), [ABI роутера](https://github.com/hashflownetwork/x-protocol/blob/main/evm/abi/IHashflowRouter.json) и [ABI пула](https://github.com/hashflownetwork/x-protocol/blob/main/evm/abi/IHashflowPool.json) определяют источник и формат проверки. Живые ответы `price-levels` и `rfq` требуют доступа владельца. В этой записи представлены только результаты исторических сделок других участников.
