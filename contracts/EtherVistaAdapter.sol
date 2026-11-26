// SPDX-License-Identifier: MIT
pragma solidity 0.8.37;

// Шаг swapFrom V5. Котируемый вход и выход — ERC-20; ETH для комиссии продажи
// предоставляет владелец модуля, и каждая успешная продажа восстанавливает его остаток.
interface IVistaRouter {
    function factory() external view returns (address);
    function WETH() external view returns (address);
    function usdcToEth(uint256 amount) external view returns (uint256);
    function swapExactETHForTokensSupportingFeeOnTransferTokens(
        uint256 minimum, address[] calldata path, address recipient, uint256 deadline
    ) external payable;
    function swapExactTokensForETHSupportingFeeOnTransferTokens(
        uint256 amount, uint256 minimum, address[] calldata path, address recipient, uint256 deadline
    ) external payable;
}

interface IVistaFactory {
    function router() external view returns (address);
    function getPair(address tokenIn, address tokenOut) external view returns (address);
}

interface IVistaPair {
    function factory() external view returns (address);
    function buyTotalFee() external view returns (uint256);
    function sellTotalFee() external view returns (uint256);
}

interface IVistaToken {
    function balanceOf(address who) external view returns (uint256);
    function allowance(address payer, address spender) external view returns (uint256);
    function approve(address spender, uint256 amount) external returns (bool);
    function transferFrom(address payer, address to, uint256 amount) external returns (bool);
}

interface IVistaWeth is IVistaToken {
    function deposit() external payable;
    function withdraw(uint256 amount) external;
    function transfer(address to, uint256 amount) external returns (bool);
}

contract EtherVistaAdapter {
    address public immutable owner;
    IVistaRouter public immutable router;
    IVistaFactory public immutable factory;
    IVistaWeth public immutable weth;
    uint256 public feeReserve;
    bool transient private entered;
    bool transient private selling;
    uint256 transient private routerPayment;

    constructor(IVistaRouter market, address reserveOwner) {
        require(address(market).code.length != 0 && reserveOwner != address(0), "configuration");
        router = market;
        factory = IVistaFactory(market.factory());
        weth = IVistaWeth(market.WETH());
        require(address(weth).code.length != 0 && factory.router() == address(market), "router");
        owner = reserveOwner;
    }

    receive() external payable {
        if (entered && !selling && msg.sender == address(weth)) return;
        require(entered && selling && msg.sender == address(router), "source");
        routerPayment += msg.value;
    }

    function fundReserve() external payable {
        require(msg.sender == owner && !entered && msg.value != 0, "owner");
        feeReserve += msg.value;
    }

    function withdrawReserve(uint256 amount) external {
        require(msg.sender == owner && !entered && amount <= feeReserve, "reserve");
        entered = true;
        feeReserve -= amount;
        (bool ok,) = payable(owner).call{value:amount}("");
        require(ok, "repayment");
        entered = false;
    }

    function swapFrom(address tokenIn, address tokenOut, uint256 amount, uint256 minimum,
                      address recipient, uint256 deadline) external returns (uint256 received) {
        require(!entered && block.timestamp <= deadline && amount != 0 && tokenIn != tokenOut, "order");
        require(recipient != address(0) && recipient != address(this) && factory.router() == address(router), "route");
        entered = true;
        address[] memory path = new address[](2);
        path[0] = tokenIn;
        path[1] = tokenOut;
        IVistaPair pair = IVistaPair(factory.getPair(tokenIn, tokenOut));
        require(address(pair) != address(0) && pair.factory() == address(factory), "market");
        bool buy = tokenIn == address(weth);
        require(buy || tokenOut == address(weth), "weth");
        IVistaToken input = IVistaToken(tokenIn);
        IVistaToken output = IVistaToken(tokenOut);
        uint256 balance = input.balanceOf(address(this));
        uint256 payerBefore = input.balanceOf(msg.sender);
        uint256 beforeOut = output.balanceOf(recipient);
        checked(tokenIn, abi.encodeCall(input.transferFrom,(msg.sender,address(this),amount)));
        uint256 actual = input.balanceOf(address(this)) - balance;
        require(actual > 0 && actual <= amount && payerBefore-input.balanceOf(msg.sender) == amount, "spent");
        if (buy) {
            require(actual == amount && amount > router.usdcToEth(pair.buyTotalFee()), "buy fee");
            weth.withdraw(amount);
            router.swapExactETHForTokensSupportingFeeOnTransferTokens{value:amount}(minimum,path,recipient,deadline);
        } else {
            sell(input, pair, path, actual, minimum, recipient, deadline);
        }
        require(input.balanceOf(address(this)) == balance && address(this).balance >= feeReserve, "residue");
        received = output.balanceOf(recipient) - beforeOut;
        require(received > 0 && received >= minimum, "payout");
        entered = false;
    }

    function sell(IVistaToken input, IVistaPair pair, address[] memory path, uint256 amount,
                  uint256 minimum, address recipient, uint256 deadline) private {
        uint256 fee = router.usdcToEth(pair.sellTotalFee());
        require(fee <= feeReserve && input.allowance(address(this),address(router)) == 0, "fee/allowance");
        uint256 dust = weth.balanceOf(address(router));
        uint256 pairBefore = weth.balanceOf(address(pair));
        checked(address(input), abi.encodeCall(input.approve,(address(router),amount)));
        selling = true;
        routerPayment = 0;
        router.swapExactTokensForETHSupportingFeeOnTransferTokens{value:fee}(
            amount, dust + fee + minimum, path, address(this), deadline
        );
        uint256 gross = routerPayment;
        routerPayment = 0;
        selling = false;
        if (input.allowance(address(this),address(router)) != 0)
            checked(address(input), abi.encodeCall(input.approve,(address(router),0)));
        require(input.allowance(address(this),address(router)) == 0 && gross > dust + fee, "settlement");
        require(weth.balanceOf(address(router)) == 0 && gross - dust == pairBefore - weth.balanceOf(address(pair)), "pool payout");
        uint256 net = gross - dust - fee;
        require(net >= minimum, "floor");
        weth.deposit{value:net+dust}();
        if (dust != 0) require(weth.transfer(address(router),dust), "router balance");
        require(weth.balanceOf(address(router)) == dust && weth.transfer(recipient,net), "delivery");
    }

    function checked(address token, bytes memory data) private {
        (bool ok, bytes memory result) = token.call(data);
        require(ok && (result.length == 0 || abi.decode(result,(bool))), "erc20");
    }
}
