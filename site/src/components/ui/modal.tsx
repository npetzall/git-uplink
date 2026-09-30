import { useEffect, useRef } from "react";
import type React from "react";
import { X } from "lucide-react";

/**
 * Modal on the native <dialog>: focus trap, Esc to close, and the page behind does not scroll.
 * The body scrolls inside the dialog. Clicking the backdrop closes it.
 */
export function Modal({
  open,
  onClose,
  title,
  actions,
  children,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  actions?: React.ReactNode;
  children: React.ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    if (!open && dialog.open) dialog.close();
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const previous = document.documentElement.style.overflow;
    document.documentElement.style.overflow = "hidden";
    return () => {
      document.documentElement.style.overflow = previous;
    };
  }, [open]);

  return (
    <dialog
      ref={ref}
      aria-label={title}
      onClose={onClose}
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
      className="m-auto w-[min(56rem,calc(100vw-2rem))] max-w-none overflow-hidden rounded-xl border border-border bg-background p-0 text-foreground shadow-2xl backdrop:bg-black/70 backdrop:backdrop-blur-sm"
    >
      <div className="flex max-h-[85vh] flex-col">
        <header className="flex flex-wrap items-center gap-3 border-b border-border px-5 py-3">
          <h2 className="me-auto text-base font-semibold">{title}</h2>
          {actions}
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="rounded-md p-1.5 text-muted-foreground hover:bg-muted hover:text-foreground focus-visible:outline-2 focus-visible:outline-primary"
          >
            <X className="size-4" />
          </button>
        </header>
        <div className="overflow-y-auto overscroll-contain px-5 py-4">{children}</div>
      </div>
    </dialog>
  );
}
