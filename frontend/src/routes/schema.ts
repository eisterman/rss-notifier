import { z } from "zod";

export const formSchema = z.object({
  name: z.string().min(2).max(50),
  feed_url: z.string().url(),
});

export const modifyFormSchema = formSchema.extend({
  id: z.number(),
});

