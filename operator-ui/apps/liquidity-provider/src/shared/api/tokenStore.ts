let token: string | null = null;

export const getToken = (): string | null => token;

export const setToken = (value: string): void => {
  token = value;
};

export const clearToken = (): void => {
  token = null;
};
