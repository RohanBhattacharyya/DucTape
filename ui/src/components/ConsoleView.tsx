import { useLayoutEffect, useRef } from "react";
import type { CSSProperties } from "react";
import type { ConsoleLine } from "../protocol";

interface Props {
  lines: ConsoleLine[];
  /** Static lines shown instead of the console, e.g. an explanation of a red indicator */
  override?: string[] | null;
  style?: CSSProperties;
  className?: string;
}

/** Robot console output that sticks to the bottom unless the user scrolls up. */
export function ConsoleView({ lines, override, style, className }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  useLayoutEffect(() => {
    const el = ref.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [lines, override]);

  const onScroll = () => {
    const el = ref.current;
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
  };

  return (
    <div ref={ref} className={`console ${className ?? ""}`} style={style} onScroll={onScroll}>
      {override
        ? override.map((text, i) => (
            <div key={i} className="line">
              {text}
            </div>
          ))
        : lines.map((line) => (
            <div key={line.seq} className={`line ${line.kind}`}>
              {line.text}
            </div>
          ))}
    </div>
  );
}
