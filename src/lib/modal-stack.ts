type CloseHandler = () => void | Promise<void>;

const stack: CloseHandler[] = [];

/**
 * Registers a modal sheet's close handler while it is mounted.
 * Returns a cleanup function to unregister it on unmount.
 */
export function registerModal(close: CloseHandler): () => void {
  stack.push(close);
  return () => {
    const i = stack.lastIndexOf(close);
    if (i !== -1) stack.splice(i, 1);
  };
}

/**
 * Closes the topmost registered modal sheet.
 * Returns true if a modal handled the close, false if none are open.
 */
export function closeTopModal(): boolean {
  const top = stack[stack.length - 1];
  if (!top) return false;
  void top();
  return true;
}
