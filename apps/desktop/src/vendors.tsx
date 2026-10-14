// What each CLI can do with anchorleg (`anchorleg vendors --json`), shared with every view so
// they offer only what a task's CLI supports. Until it loads (or with an older CLI), everything
// is offered, as before.
import { createContext, useContext } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Account } from "./api";

export interface VendorCaps {
  vendor: string;
  label: string;
  driven: boolean;
  quota: "live" | "runs" | "none";
  resume: boolean;
  fork: boolean;
  side: boolean;
  rewind: boolean;
  plan: boolean;
  approvals: boolean;
  questions: boolean;
  json_schema: boolean;
  instructions: string[];
  folder_env: string | null;
  folder_prefix: string | null;
  sign_in: { kind: "browser" | "terminal" | "manual" };
  bin: string;
  installed: boolean;
  efforts: string[] | null;
}

export const loadVendors = () => invoke<{ schema: number; vendors: VendorCaps[] }>("vendors").then((r) => r.vendors);

const FALLBACK: Record<string, string> = { claude: "Claude Code", codex: "Codex", antigravity: "Antigravity", cursor: "Cursor", kimi: "Kimi" };

export const VendorsContext = createContext<{ vendors: VendorCaps[]; accounts: Account[] }>({ vendors: [], accounts: [] });

export function useVendors() {
  const { vendors, accounts } = useContext(VendorsContext);
  const caps = (vendor: string | undefined | null) => vendors.find((v) => v.vendor === vendor);
  return {
    vendors,
    /** The CLIs anchorleg can run tasks on. */
    driven: vendors.length ? vendors.filter((v) => v.driven) : Object.keys(FALLBACK).slice(0, 3).map((v) => ({ vendor: v, label: FALLBACK[v] }) as VendorCaps),
    label: (vendor: string) => caps(vendor)?.label ?? FALLBACK[vendor] ?? vendor,
    caps,
    /** The caps of the CLI an account runs; undefined when unknown (then everything is offered). */
    ofAccount: (account: string | null | undefined) => caps(accounts.find((a) => a.name === account)?.vendor),
  };
}
