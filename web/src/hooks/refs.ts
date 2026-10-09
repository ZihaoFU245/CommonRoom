/** State refs always have a value; DOM refs use Preact's nullable RefObject. */
export interface ValueRef<T> {
  current: T;
}
