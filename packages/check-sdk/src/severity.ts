// Mirror of cofferdam_core::Severity. Wire form is the lowercase string;
// the const-as-namespace pattern keeps `Severity.Warning` ergonomic
// without spinning up an enum (which TS enums emit runtime code for).

export const Severity = {
  Info: "info",
  Low: "low",
  Medium: "medium",
  High: "high",
  Critical: "critical",
} as const;

export type Severity = (typeof Severity)[keyof typeof Severity];
