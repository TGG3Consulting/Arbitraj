// SPDX-License-Identifier: MIT
// Обмен WETH/SRG через собственную кривую Surge. Один плательщик и его исполнитель
// используют одну учётную запись на кривой и её штатное ограничение на продажу в блоке покупки.
// Модуль развёртывает владелец кошелька в Ethereum.
pragma solidity 0.8.37;

interface ISurgeAsset {
    function balanceOf(address who) external view returns (uint256);
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
}
interface ISurgeMarket is ISurgeAsset {
    function _buy(uint256 minimum, uint256 deadline) external payable returns (bool);
    function _sell(uint256 amount, uint256 deadline, uint256 minimumGross) external returns (bool);
}
interface ISurgeWrapped is ISurgeAsset {
    function withdraw(uint256 amount) external;
    function deposit() external payable;
}

contract SurgeAdapter {
    ISurgeMarket public immutable market;
    ISurgeWrapped public immutable wrapped;
    address public immutable payer;
    address public immutable executor;
    bool transient private entered;

    event Filled(address indexed sender, address indexed recipient, address indexed input,
                 address output, uint256 spent, uint256 received);

    constructor(ISurgeMarket token, ISurgeWrapped weth, address wallet, address execution) {
        require(block.chainid == 1 && address(token) != address(weth) &&
                address(token).code.length != 0 && address(weth).code.length != 0, "market");
        require(wallet != address(0) && wallet != address(this) && execution != wallet, "wallet");
        market = token;
        wrapped = weth;
        payer = wallet;
        executor = execution;
    }

    receive() external payable {
        require(msg.sender == address(wrapped) || msg.sender == address(market), "sender");
    }

    // Wrapped denotes the native side in the ABI. Only the payer can settle ETH to its own wallet.
    function swapNative(address input, address output, uint256 amount, uint256 minimum,
                        address recipient, uint256 deadline) external payable returns (uint256 received) {
        require(!entered && msg.sender == payer && recipient == payer, "payer");
        require(block.timestamp <= deadline && amount != 0 && minimum != 0, "order");
        bool buy = input == address(wrapped) && output == address(market);
        require(buy || (input == address(market) && output == address(wrapped)), "pair");
        require(msg.value == (buy ? amount : 0), "value");
        entered = true;
        uint256 initialEth = address(this).balance - msg.value;
        uint256 initialTokens = market.balanceOf(address(this));
        uint256 initialWallet = market.balanceOf(payer);
        uint256 initialWalletEth = payer.balance;

        if (buy) {
            require(market._buy{value:amount}(minimum, deadline), "buy");
            uint256 tokens = market.balanceOf(address(this)) - initialTokens;
            require(market.transfer(payer, tokens), "delivery");
            received = market.balanceOf(payer) - initialWallet;
            require(received == tokens && payer.balance == initialWalletEth, "net tokens");
        } else {
            require(market.transferFrom(payer, address(this), amount) &&
                    market.balanceOf(address(this)) - initialTokens == amount, "funding");
            require(market._sell(amount, deadline, minimum), "sell");
            uint256 earned = address(this).balance - initialEth;
            require(earned >= minimum && initialWallet - market.balanceOf(payer) == amount, "net ETH");
            (bool paid,) = payer.call{value:earned}("");
            require(paid, "delivery");
            received = payer.balance - initialWalletEth;
            require(received == earned, "net ETH");
        }
        require(received >= minimum && market.balanceOf(address(this)) == initialTokens &&
                address(this).balance == initialEth, "settlement");
        emit Filled(payer, payer, input, output, amount, received);
        entered = false;
    }

    function swapFrom(address input, address output, uint256 amount, uint256 minimum,
                      address recipient, uint256 deadline) external returns (uint256 received) {
        require(!entered && (msg.sender == payer || msg.sender == executor), "payer");
        require(block.timestamp <= deadline && amount != 0, "order");
        require(recipient != address(0) && recipient != address(this) &&
                recipient != address(market) && recipient != address(wrapped), "recipient");
        bool buy = input == address(wrapped) && output == address(market);
        require(buy || (input == address(market) && output == address(wrapped)), "pair");
        entered = true;

        uint256 initialEth = address(this).balance;
        ISurgeAsset src = ISurgeAsset(input);
        ISurgeAsset dst = ISurgeAsset(output);
        uint256 initialInput = src.balanceOf(address(this));
        uint256 initialOutput = dst.balanceOf(address(this));
        uint256 payerBalance = src.balanceOf(msg.sender);
        uint256 recipientBalance = dst.balanceOf(recipient);
        require(src.transferFrom(msg.sender, address(this), amount) &&
                src.balanceOf(address(this)) - initialInput == amount, "funding");

        if (buy) {
            wrapped.withdraw(amount);
            require(address(this).balance - initialEth == amount, "unwrap");
            require(market._buy{value: amount}(minimum, deadline), "buy");
        } else {
            require(market._sell(amount, deadline, minimum), "sell");
            uint256 paid = address(this).balance - initialEth;
            require(paid != 0 && paid >= minimum, "net ETH");
            wrapped.deposit{value: paid}();
        }
        uint256 delivery = dst.balanceOf(address(this)) - initialOutput;
        require(delivery != 0 && dst.transfer(recipient, delivery), "delivery");
        received = dst.balanceOf(recipient) - recipientBalance;
        require(received == delivery && received >= minimum, "net");
        require(src.balanceOf(address(this)) == initialInput && dst.balanceOf(address(this)) == initialOutput &&
                address(this).balance == initialEth && payerBalance - src.balanceOf(msg.sender) == amount, "settlement");
        emit Filled(msg.sender, recipient, input, output, amount, received);
        entered = false;
    }
}
