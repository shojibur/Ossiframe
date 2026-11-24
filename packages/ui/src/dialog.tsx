import * as D from "@radix-ui/react-dialog";
import type { ReactNode } from "react";
import { cn } from "./cn";

interface DialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  className?: string;
}

export function Dialog({ open, onOpenChange, title, description, children, footer, className }: DialogProps) {
  return (
    <D.Root open={open} onOpenChange={onOpenChange}>
      <D.Portal>
        <D.Overlay className="fixed inset-0 bg-black/40" />
        <D.Content
          className={cn(
            "fixed top-1/2 left-1/2 flex max-h-[90vh] w-[min(560px,92vw)] -translate-x-1/2 -translate-y-1/2 flex-col gap-4 overflow-y-auto rounded-lg border border-slate-200 bg-white p-5 shadow-lg dark:border-slate-800 dark:bg-slate-900",
            className,
          )}
        >
          <div className="flex flex-col gap-1">
            <D.Title className="text-base font-semibold text-slate-900 dark:text-slate-100">{title}</D.Title>
            {description ? (
              <D.Description className="text-sm text-slate-600 dark:text-slate-400">{description}</D.Description>
            ) : (
              <D.Description className="sr-only">{title}</D.Description>
            )}
          </div>
          {children}
          {footer && <div className="flex justify-end gap-2">{footer}</div>}
        </D.Content>
      </D.Portal>
    </D.Root>
  );
}
