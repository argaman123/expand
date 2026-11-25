import { Link } from "@fluentui/react-components";

/** What failed, and how to run it again. */
export type Failure = { text: string; retry: () => void };

export const failureText = (e: unknown) => String(e instanceof Error ? e.message : e);

/** An inline error with a Retry that runs the failed action again: never a dead end. */
export function ErrorLine({ failure, className }: { failure: Failure; className: string }) {
  return (
    <div className={`ex-err-line ${className}`} title={failure.text}>
      <span className="ex-err-txt">{failure.text}</span>
      <Link as="button" className="ex-retry" onClick={failure.retry}>Retry</Link>
    </div>
  );
}
