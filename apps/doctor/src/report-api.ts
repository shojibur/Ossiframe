// Typed wrappers for the Doctor-only report commands (apps/doctor/src-tauri/src/report_commands.rs).
// Mirrors `ReportInput` in modules/linux-audit/src/report.rs. Keep in sync with the Rust definitions.
// The frontend never supplies a file path or file content: Rust opens the save dialog.
import { invoke } from "@tauri-apps/api/core";
import { toIpcError } from "@ossiframe/client";
import type { CheckResult, StepResult } from "./audit-types";

export type ReportFormat = "json" | "html";

export interface ReportServer {
  profileName: string;
  /** The configured connection address. Omitted from the report when `redactHost` is set. */
  host: string;
  port: number;
  hostname: string | null;
  os: string | null;
  kernel: string | null;
}

export interface ReportScan {
  startedAt: string;
  finishedAt: string | null;
  /** Steps that ran to the end. */
  completedSteps: string[];
  stale: boolean;
}

export interface ReportInput {
  server: ReportServer;
  scan: ReportScan;
  steps: StepResult[];
  redactHost: boolean;
}

export type { CheckResult };

interface ReportRequest {
  format: ReportFormat;
  report: ReportInput;
}

async function call<T>(cmd: string, request: ReportRequest): Promise<T> {
  try {
    return await invoke<T>(cmd, { request });
  } catch (e) {
    throw toIpcError(e);
  }
}

/** Builds the report text for the preview. No I/O. */
export const buildReport = (format: ReportFormat, report: ReportInput) =>
  call<string>("build_report", { format, report });

/** Saves the report where the user chooses. Resolves `false` when the user cancels the dialog. */
export const saveReport = (format: ReportFormat, report: ReportInput) =>
  call<boolean>("save_report", { format, report });
