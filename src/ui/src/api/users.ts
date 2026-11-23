import type { User } from "./types";

/** The name a user signs in with: username, else email. */
export const loginOf = (u: Pick<User, "id" | "username" | "email">) => u.username ?? u.email ?? u.id.slice(0, 8);
