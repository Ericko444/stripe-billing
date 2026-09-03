import { useState } from "react";
import { ApiProblem } from "../../host/api/apiClient";
import { useInvoices } from "../hooks/useInvoices";
import { formatMoney } from "../../money";
import { formatDate } from "../../format";
import { Alert, Badge, Button, Card, EmptyState, Loading, Row } from "../../ui/primitives";
import type { InvoiceStatus } from "../api/types";

const STATUS_TONE: Record<InvoiceStatus, "green" | "amber" | "red"> = {
  paid: "green",
  open: "amber",
  failed: "red",
};

/** U3, 4b's keyset pagination -- pages by `next`, never an offset. "Load
 * more" replaces the visible page rather than accumulating; per P4 this is
 * the first thing cut under time pressure, so it stays deliberately simple. */
export function InvoiceList() {
  const [after, setAfter] = useState<string | undefined>(undefined);
  const query = useInvoices(after);

  if (query.isPending) {
    return (
      <Card title="Invoices">
        <Loading>Loading invoices…</Loading>
      </Card>
    );
  }

  if (query.isError) {
    const detail =
      query.error instanceof ApiProblem ? query.error.detail : "Something went wrong.";
    return (
      <Card title="Invoices">
        <Alert>Could not load invoices: {detail}</Alert>
      </Card>
    );
  }

  const page = query.data;

  if (page.items.length === 0) {
    return (
      <Card title="Invoices">
        <EmptyState>No invoices yet.</EmptyState>
      </Card>
    );
  }

  return (
    <Card title="Invoices">
      <div className="space-y-3">
        <ul className="space-y-2">
          {page.items.map((invoice) => (
            <li key={invoice.id}>
              <Row>
                <div>
                  <p className="font-medium tabular-nums text-slate-900">
                    {formatMoney(invoice.amount)}
                  </p>
                  <p className="mt-0.5 text-xs text-slate-500">{formatDate(invoice.created_at)}</p>
                </div>
                <Badge tone={STATUS_TONE[invoice.status]}>{invoice.status}</Badge>
              </Row>
            </li>
          ))}
        </ul>
        {page.next !== undefined && (
          <Button variant="ghost" onClick={() => setAfter(page.next)}>
            Load more
          </Button>
        )}
      </div>
    </Card>
  );
}
