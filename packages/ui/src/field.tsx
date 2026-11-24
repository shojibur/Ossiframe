import { useId, type InputHTMLAttributes, type ReactNode } from "react";
import { cn } from "./cn";

export function Input({ className, ...props }: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      className={cn(
        "h-9 w-full rounded-md border border-slate-300 bg-white px-2.5 text-sm text-slate-900 placeholder:text-slate-400 focus-visible:outline-2 focus-visible:outline-sky-600 aria-invalid:border-red-600 dark:border-slate-700 dark:bg-slate-950 dark:text-slate-100",
        className,
      )}
      {...props}
    />
  );
}

interface FieldProps {
  label: string;
  error?: string | undefined;
  hint?: ReactNode;
  children: (props: { id: string; "aria-invalid": boolean; "aria-describedby"?: string }) => ReactNode;
}

/** Label, control and an error message linked for screen readers. */
export function Field({ label, error, hint, children }: FieldProps) {
  const id = useId();
  const errId = `${id}-error`;
  return (
    <div className="flex flex-col gap-1">
      <label htmlFor={id} className="text-xs font-medium text-slate-700 dark:text-slate-300">
        {label}
      </label>
      {children({ id, "aria-invalid": Boolean(error), ...(error ? { "aria-describedby": errId } : {}) })}
      {hint && !error && <p className="text-xs text-slate-500">{hint}</p>}
      {error && (
        <p id={errId} className="text-xs text-red-700 dark:text-red-400">
          {error}
        </p>
      )}
    </div>
  );
}
