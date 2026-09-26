/**
 * Every identity query key starts with `"identity"`, so the host can clear
 * everything *else* on a tenant switch without this feature knowing what
 * else exists.
 */
export const identityKeys = {
  all: ["identity"] as const,
  session: ["identity", "session"] as const,
};
