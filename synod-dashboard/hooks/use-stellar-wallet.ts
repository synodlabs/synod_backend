import { useEffect, useState } from 'react';
import {
  StellarWalletsKit,
  Networks,
} from '@creit.tech/stellar-wallets-kit';
import { defaultModules } from '@creit.tech/stellar-wallets-kit/modules/utils';

export interface UseStellarWallet {
  address: string | null;
  connect: () => Promise<string | null>;
  sign: (message: string, walletAddress?: string | null) => Promise<string | null>;
  disconnect: () => void;
}

// Ensure the module only initializes once globally across all hook instances
let isKitInitialized = false;

const normalizeWalletError = (err: unknown): string => {
  if (!err) {
    return "Wallet connection failed";
  }

  if (err instanceof Error) {
    return err.message || "Wallet connection failed";
  }

  if (typeof err === "string") {
    return err;
  }

  if (typeof err === "object") {
    const candidate = err as { message?: unknown; code?: unknown; name?: unknown };
    const parts = [candidate.name, candidate.code, candidate.message]
      .filter((value) => typeof value === "string" || typeof value === "number")
      .map((value) => String(value).trim())
      .filter(Boolean);

    if (parts.length > 0) {
      return parts.join(": ");
    }
  }

  return "Wallet connection failed";
};

export const useStellarWallet = (): UseStellarWallet => {
  const [address, setAddress] = useState<string | null>(null);

  useEffect(() => {
    if (typeof window !== 'undefined') {
      const savedAddress = localStorage.getItem('synod_wallet_address');
      if (savedAddress) {
        setAddress(savedAddress);
      }

      if (!isKitInitialized) {
        try {
          StellarWalletsKit.init({
            network: Networks.TESTNET,
            modules: defaultModules(),
            authModal: {
              hideUnsupportedWallets: true,
              showInstallLabel: true,
            },
          });
          isKitInitialized = true;
        } catch (e) {
          console.error("Failed to initialize StellarWalletsKit", e);
        }
      }
    }
  }, []);

  const connect = async () => {
    try {
      const { address } = await StellarWalletsKit.authModal();
      setAddress(address);
      localStorage.setItem('synod_wallet_address', address);
      return address;
    } catch (err: unknown) {
      const msg = normalizeWalletError(err);
      const code = typeof err === "object" && err !== null && "code" in err ? (err as { code?: unknown }).code : undefined;

      if (msg.toLowerCase().includes("rejected") || code === -4) {
        console.warn("Wallet connection cancelled by user");
        return null;
      } else {
        console.error("Wallet connection failed:", msg, err);
        throw new Error(msg);
      }
    }
  };

  const sign = async (message: string, walletAddress?: string | null) => {
    const activeAddress = walletAddress || address;
    if (!activeAddress) return null;
    try {
      const { signedMessage } = await StellarWalletsKit.signMessage(message, {
        networkPassphrase: Networks.TESTNET,
        address: activeAddress,
      });
      if (!signedMessage) throw new Error("No signature returned");
      return signedMessage;
    } catch (err: unknown) {
      const msg = normalizeWalletError(err);
      console.error("Signing failed:", msg, err);
      // Handle "User rejected" specifically if we can detect the code/message
      const code = typeof err === "object" && err !== null && "code" in err ? (err as { code?: unknown }).code : undefined;
      if (msg.toLowerCase().includes("rejected") || code === -4) {
        throw new Error("Wallet signing rejected by user");
      }
      throw new Error(msg || "Signing failed");
    }
  };

  const disconnect = () => {
    try {
      StellarWalletsKit.disconnect();
    } catch { }
    localStorage.removeItem('synod_wallet_address');
    setAddress(null);
  };

  return { address, connect, sign, disconnect };
};
