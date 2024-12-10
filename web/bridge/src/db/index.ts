import { drizzle } from "drizzle-orm/postgres-js";

const postgresConfig = process.env.POSTGRES_URL || "postgres://user:password@localhost:5432/postgres"

const db = drizzle(postgresConfig);

export const build_data = await db.execute('select * from build_data');

export { db };
