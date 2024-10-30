type Pending = {
  done: false;
};
type Succeed<T> = {
  done: true;
  succeed: true;
  data: T;
};
type Failed = {
  done: true;
  succeed: false;
  error: unknown;
};
export type QueryResult<T> = Pending | Succeed<T> | Failed;
export const pending = (): Pending => ({ done: false });
export const failed = (error: unknown): Failed => ({
  done: true,
  succeed: false,
  error,
});
export const succeed = <T>(data: T): Succeed<T> => ({
  done: true,
  succeed: true,
  data,
});
