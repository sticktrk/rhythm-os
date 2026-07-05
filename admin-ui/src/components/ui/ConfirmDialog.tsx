import {
  createContext,
  useCallback,
  useContext,
  useRef,
  useState,
  type ReactNode
} from 'react';
import { AlertTriangle } from 'lucide-react';

import { Modal } from './Modal';

export type ConfirmOptions = {
  title: string;
  message: string;
  confirmLabel?: string;
  danger?: boolean;
  /** For the worst operations (factory reset, backup restore): the user
      must type this phrase before Confirm enables. */
  requireTypedText?: string;
};

type ConfirmFn = (options: ConfirmOptions) => Promise<boolean>;

const ConfirmContext = createContext<ConfirmFn | null>(null);

export function ConfirmProvider({ children }: { children: ReactNode }) {
  const [options, setOptions] = useState<ConfirmOptions | null>(null);
  const [typed, setTyped] = useState('');
  const resolverRef = useRef<((value: boolean) => void) | null>(null);

  const confirm = useCallback<ConfirmFn>((next) => {
    // A concurrent confirm replaces the pending one; settle it as declined
    // so its caller never hangs on an orphaned promise.
    resolverRef.current?.(false);
    setOptions(next);
    setTyped('');
    return new Promise<boolean>((resolve) => {
      resolverRef.current = resolve;
    });
  }, []);

  function settle(value: boolean) {
    resolverRef.current?.(value);
    resolverRef.current = null;
    setOptions(null);
    setTyped('');
  }

  const typedOk =
    !options?.requireTypedText || typed.trim() === options.requireTypedText;

  return (
    <ConfirmContext.Provider value={confirm}>
      {children}
      {options ? (
        <Modal title={options.title} onClose={() => settle(false)}>
          <div className="confirmBody">
            {options.danger ? (
              <div className="confirmIcon danger">
                <AlertTriangle size={22} />
              </div>
            ) : null}
            <p>{options.message}</p>
            {options.requireTypedText ? (
              <label className="confirmTyped">
                <span>
                  Type <code>{options.requireTypedText}</code> to continue
                </span>
                <input
                  value={typed}
                  onChange={(event) => setTyped(event.target.value)}
                  spellCheck={false}
                  autoComplete="off"
                />
              </label>
            ) : null}
            <div className="confirmActions">
              <button
                className="consoleButton"
                type="button"
                onClick={() => settle(false)}
              >
                Cancel
              </button>
              <button
                className={`consoleButton primary${options.danger ? ' danger' : ''}`}
                type="button"
                disabled={!typedOk}
                onClick={() => settle(true)}
              >
                {options.confirmLabel ?? 'Confirm'}
              </button>
            </div>
          </div>
        </Modal>
      ) : null}
    </ConfirmContext.Provider>
  );
}

export function useConfirm(): ConfirmFn {
  const confirm = useContext(ConfirmContext);
  if (!confirm) {
    throw new Error('useConfirm must be used within ConfirmProvider');
  }
  return confirm;
}
