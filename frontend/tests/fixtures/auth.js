export const issuerUrl = "https://gitlab.com";
export const issuer = {
  issuer: "https://gitlab.com",
  loginUrl: `https://example/auth/login?issuer=${encodeURIComponent(
    issuerUrl
  )}`,
};
export const issuers = [issuer];

export const me = {
  sub: 123,
  iss: issuer.issuer,
  name: "John Doe"
};
