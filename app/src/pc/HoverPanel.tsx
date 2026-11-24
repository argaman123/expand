import { cloneElement, useEffect, useRef, useState, type FocusEvent, type ReactElement, type ReactNode } from "react";
import { Popover, PopoverSurface, PopoverTrigger } from "@fluentui/react-components";

/** A panel that opens above its trigger on hover, or while keyboard focus is in the trigger or the panel.
 *  A click on the trigger never opens or closes it. One panel shows at a time: opening says so
 *  (`onOpen`), and a panel that is no longer `current` closes at once, without its leave delay. */
export function HoverPanel({ trigger, children, label, current, onOpen }: {
  trigger: ReactElement;
  children: ReactNode;
  label: string;
  current: boolean;
  onOpen: () => void;
}) {
  const [hover, setHover] = useState(false);
  const [focus, setFocus] = useState(false);
  const open = hover || focus;
  const opened = useRef(onOpen);
  opened.current = onOpen;
  useEffect(() => {
    if (open) opened.current();
  }, [open]);
  useEffect(() => {
    if (current) return;
    setHover(false);
    setFocus(false);
  }, [current]);
  const triggerEl = useRef<HTMLElement | null>(null);
  const surfaceEl = useRef<HTMLDivElement | null>(null);

  const leave = (e: FocusEvent) => {
    const to = e.relatedTarget as Node | null;
    if (to && (triggerEl.current?.contains(to) || surfaceEl.current?.contains(to))) return;
    setFocus(false);
  };
  const tile = cloneElement(trigger, {
    ref: triggerEl,
    onFocus: (e: FocusEvent<HTMLElement>) => {
      if (e.currentTarget.matches(":focus-visible")) setFocus(true); // keyboard focus only, not a mouse press
    },
    onBlur: leave,
  } as object);

  return (
    <Popover
      open={open && current}
      openOnHover
      mouseLeaveDelay={250}
      positioning={{ position: "above", offset: 4 }}
      onOpenChange={(e, d) => {
        if (e.type === "click") return;
        setHover(d.open);
        if (!d.open) setFocus(false);
      }}
    >
      <PopoverTrigger disableButtonEnhancement>{tile}</PopoverTrigger>
      <PopoverSurface ref={surfaceEl} className="ss-panel" aria-label={label} onBlur={leave}>{children}</PopoverSurface>
    </Popover>
  );
}
