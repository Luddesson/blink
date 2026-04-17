const { Alchemy, Network } = require("alchemy-sdk");

const config = {
  apiKey: "demo", // Free public endpoint
  network: Network.MATIC_MAINNET,
};

const alchemy = new Alchemy(config);
const walletAddress = "0x2005d16a84ceefa912d4e380cd32e7ff827875ea";

async function getWalletActivity() {
  try {
    console.log("Fetching transactions for wallet:", walletAddress);
    
    // Get transaction history
    const history = await alchemy.core.getAssetTransfers({
      fromAddress: walletAddress,
      category: ["external", "internal", "erc20", "erc721", "erc1155"],
      maxCount: 100,
    });
    
    console.log(`Found ${history.transfers.length} outgoing transactions`);
    console.log(JSON.stringify(history.transfers.slice(0, 5), null, 2));
    
    // Get incoming transactions
    const incoming = await alchemy.core.getAssetTransfers({
      toAddress: walletAddress,
      category: ["external", "internal", "erc20", "erc721", "erc1155"],
      maxCount: 100,
    });
    
    console.log(`Found ${incoming.transfers.length} incoming transactions`);
    
  } catch (error) {
    console.error("Error:", error.message);
  }
}

getWalletActivity();
