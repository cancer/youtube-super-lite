type Succeed<T> = {
  isSuccess: true;
  isError: false;
  data: T;
  error: null;
};
type Failed<E> = {
  isSuccess: false;
  isError: true;
  data: null;
  error: E;
};
export type Result<T = unknown, E = unknown> = Succeed<T> | Failed<E>;

// Since exceptions thrown inside cache() are not caught by ErrorBoundary
export const result = async (
  fetcher: () => Promise<any>,
  logger: { log: (value: unknown) => void },
): Promise<Result<Awaited<ReturnType<typeof fetcher>>>> => {
  try {
    const data = await fetcher();
    return {
      isSuccess: true,
      isError: false,
      data,
      error: null,
    };
  } catch (error) {
    logger.log(error);
    return {
      isSuccess: false,
      isError: true,
      data: null,
      error,
    };
  }
};
