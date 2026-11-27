import { type KeyboardEvent, type ReactNode, useEffect, useRef } from "react";

interface ModalProps {
  title: string;
  role?: "dialog" | "alertdialog";
  testId: string;
  onClose: () => void;
  children: ReactNode;
  /** Describes the dialog, for screen readers. */
  describedBy?: string;
}

const FOCUSABLE =
  'button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex="-1"]), textarea, select';

/**
 * A dialog over the window. Escape or a click outside closes it, Tab stays
 * inside it, and the focus goes back where it was when it closes.
 */
export function Modal({
  title,
  role = "dialog",
  testId,
  onClose,
  children,
  describedBy,
}: ModalProps) {
  const dialog = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    const element = dialog.current;
    if (element && !element.contains(document.activeElement)) {
      const preferred = element.querySelector<HTMLElement>("[data-autofocus]");
      (preferred ?? element.querySelector<HTMLElement>(FOCUSABLE))?.focus();
    }
    return () => {
      if (previous?.isConnected) previous.focus();
    };
  }, []);

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onClose();
      return;
    }
    if (event.key !== "Tab" || !dialog.current) return;
    const focusable = [...dialog.current.querySelectorAll<HTMLElement>(FOCUSABLE)];
    const first = focusable[0];
    const last = focusable.at(-1);
    if (!first || !last) return;
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialog}
        role={role}
        aria-modal="true"
        aria-label={title}
        aria-describedby={describedBy}
        className="modal"
        data-testid={testId}
        onKeyDown={onKeyDown}
      >
        <h2 className="modal-title">{title}</h2>
        {children}
      </div>
    </div>
  );
}
