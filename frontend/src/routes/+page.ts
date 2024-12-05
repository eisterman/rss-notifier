import type { PageLoad } from './$types';  // Svelte-Kit
import type { Feed } from '$types';  // My Types

import { superValidate } from "sveltekit-superforms";
import { formSchema } from "./schema";
import { zod } from "sveltekit-superforms/adapters";

export const load: PageLoad = async ({ fetch }) => {
  // Fetch Data
  const res = await fetch('http://localhost:3000/feeds/');
  if (!res.ok) throw new Error('Bad response');
  const feeds: Feed[] = await res.json();
  return {
    feeds,
    form: await superValidate(zod(formSchema)), // Schema for Form (cached)
  };
  // If an error is thrown during load, the nearest +error.svelte will be rendered.
  // For expected errors, use the error helper from @sveltejs/kit to specify
  // the HTTP status code and an optional message:
  // import { error } from '@sveltejs/kit';
  // error(404, 'Not found');
};