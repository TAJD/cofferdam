// CD-331: type-2 clones — blocks with identical structure that differ
// only in their literal values. Refactor.DuplicateBlock should flag the
// pair below (with the "differing only in literal values" wording) even
// though every string/number literal is different. The unrelated block
// at the bottom has different structure and must NOT be flagged.
//
// The two 6-statement import runs below (CD-331 follow-up) are a second,
// separate hazard: under literal normalization, module specifiers
// collapse just like any other string literal, so two same-length import
// blocks would otherwise hash equal too — an unactionable false positive,
// since you cannot extract a shared helper out of an import list. Import
// declarations are excluded from windowing entirely, so this pair must
// NOT be flagged.
import { helperOne } from "./helpers/helper-one";
import { helperTwo } from "./helpers/helper-two";
import { helperThree } from "./helpers/helper-three";
import { helperFour } from "./helpers/helper-four";
import { helperFive } from "./helpers/helper-five";
import { helperSix } from "./helpers/helper-six";

export function chargeGold(account: Account): Receipt {
  const productId = "gold-membership";
  const amountCents = 4999;
  const description = "Gold membership";
  const invoice = createInvoice(account, productId, amountCents);
  const receipt = submitInvoice(invoice, description);
  return receipt;
}

export function chargeSilver(account: Account): Receipt {
  const productId = "silver-membership";
  const amountCents = 2999;
  const description = "Silver membership";
  const invoice = createInvoice(account, productId, amountCents);
  const receipt = submitInvoice(invoice, description);
  return receipt;
}

export function renderSummaryPanel(items: Item[]): string {
  const rows = items.map((item) => `${item.name}: ${item.count}`);
  const header = "Summary";
  const totalCount = items.reduce((sum, item) => sum + item.count, 0);
  const footer = `Total: ${totalCount}`;
  return [header, ...rows, footer].join("\n");
}

import { utilOne } from "./util/util-one";
import { utilTwo } from "./util/util-two";
import { utilThree } from "./util/util-three";
import { utilFour } from "./util/util-four";
import { utilFive } from "./util/util-five";
import { utilSix } from "./util/util-six";

declare interface Account {
  id: string;
}
declare interface Receipt {
  id: string;
}
declare interface Item {
  name: string;
  count: number;
}
declare function createInvoice(
  account: Account,
  productId: string,
  amountCents: number
): unknown;
declare function submitInvoice(invoice: unknown, description: string): Receipt;
