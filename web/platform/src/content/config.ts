import { defineCollection, z } from "astro:content";
import { docsSchema } from "@astrojs/starlight/schema";

const docs = defineCollection({ schema: docsSchema() });

const posts = defineCollection({
  type: "content",
  schema: z.object({
    title: z.string(),
    tags: z.array(z.string()),
    image: z.string().optional(),
    readTime: z.string().optional(),
    pubDate: z.date(),
  }),
});

export const collections = { docs, posts };
