// SPDX-License-Identifier: MIT
// Файл: contracts/FermiSlotExecutor.sol.
// Исполняет одно поручение Fermi на Ethereum по разрешению кошелька плательщика.
// Плательщик вызывает execute лично, получает выручку и неиспользованный остаток.
// Зависимости: проверенные роутер и оракул из реестра Titan, allowance и balanceOf ERC-20.
// Параметры: идентификатор поручения, родитель, высота, время, токены, объём и предел цены.
pragma solidity 0.8.37;

// Реестр развёртываний роутера и оракула https://docs.titanbuilder.xyz/propamms
interface IFermiSlotRouter {
    function fermi() external view returns (address);
    function fermiSwapWithAllowances(address,address,int256,uint256,address)
        external returns (uint256 amountIn,uint256 amountOut);
}

interface IFermiToken {
    function balanceOf(address user) external view returns (uint256);
    function allowance(address user, address spender) external view returns (uint256);
    function transferFrom(address from,address to,uint256 amount) external returns (bool);
    function transfer(address to,uint256 amount) external returns (bool);
    function approve(address spender,uint256 amount) external returns (bool);
}

/// Условия слота проверяются и на цепи. Поздняя копия подписанной транзакции откатывается.
/// Каждый плательщик имеет собственный набор исполненных поручений.
contract FermiSlotExecutor {
    address public constant ROUTER = 0xF6d5b2FefeC7B420d9990b95790ADFD3D4c4F977;
    address public constant ENGINE = 0x10e1A2b624e14ac59426199E520320313a98b0bE;
    bytes32 private constant ROUTER_CODE = 0x7b309d240875f4c0767cb5834a8c580f74de2f0b509068c3e83aa65321f39a54;
    struct Order {
        bytes32 leg;
        bytes32 parent;
        uint64 target;
        uint64 timestamp;
        address input;
        address output;
        int256 amount;   // Положительное значение задаёт точный вход, отрицательное — точный выход.
        uint256 limit;   // Минимальный кредит при точном входе или максимальное списание при точном выходе.
    }
    mapping(address => mapping(bytes32 => bool)) public completed;
    bool private trading;

    error WrongBlock();
    error InvalidOrder();
    error UsedLeg();
    error InvalidFill();
    event Filled(address indexed payer, bytes32 indexed leg, address indexed input,
                 address output, uint256 spent, uint256 received);

    function execute(Order calldata o) external returns (uint256 spent,uint256 received) {
        if (block.chainid != 1 || block.number != o.target || block.timestamp != o.timestamp ||
            o.parent == bytes32(0) || blockhash(block.number-1) != o.parent) revert WrongBlock();
        if (trading || o.leg == bytes32(0) || o.input == o.output || o.input == address(0) ||
            o.output == address(0) || o.amount == 0 || o.amount == type(int256).min || o.limit == 0 ||
            ROUTER.codehash != ROUTER_CODE || IFermiSlotRouter(ROUTER).fermi() != ENGINE) revert InvalidOrder();
        address payer = msg.sender;
        if (completed[payer][o.leg]) revert UsedLeg();
        completed[payer][o.leg] = true;
        trading = true;
        bool exactIn = o.amount > 0;
        uint256 budget = exactIn ? uint256(o.amount) : o.limit;
        IFermiToken token = IFermiToken(o.input);
        uint256 original = token.balanceOf(address(this));
        uint256 payerIn = token.balanceOf(payer);
        uint256 payerOut = IFermiToken(o.output).balanceOf(payer);
        checked(o.input,abi.encodeCall(token.transferFrom,(payer,address(this),budget)));
        if (token.balanceOf(address(this)) != original + budget || token.allowance(address(this),ROUTER) != 0)
            revert InvalidFill();
        checked(o.input,abi.encodeCall(token.approve,(ROUTER,budget)));
        (spent, received) = IFermiSlotRouter(ROUTER).fermiSwapWithAllowances(o.input,o.output,o.amount,o.limit,payer);
        if (spent == 0 || received == 0 || spent > budget || (exactIn ? spent != budget || received < o.limit :
            received != uint256(-o.amount))) revert InvalidFill();
        checked(o.input,abi.encodeCall(token.approve,(ROUTER,0)));
        uint256 residual = budget - spent;
        if (token.balanceOf(address(this)) != original + residual) revert InvalidFill();
        if (residual != 0) checked(o.input,abi.encodeCall(token.transfer,(payer,residual)));
        if (token.balanceOf(address(this)) != original || token.allowance(address(this),ROUTER) != 0 ||
            token.balanceOf(payer) + spent != payerIn || IFermiToken(o.output).balanceOf(payer) != payerOut + received)
            revert InvalidFill();
        trading = false;
        emit Filled(payer,o.leg,o.input,o.output,spent,received);
    }

    function checked(address token, bytes memory data) private {
        (bool success,bytes memory result) = token.call(data);
        if (!success || (result.length != 0 && !abi.decode(result,(bool)))) revert InvalidFill();
    }
}
