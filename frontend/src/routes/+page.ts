import type { PageLoad } from './$types';  // Svelte-Kit
import type { Feed } from '$types';  // My Types
import { PUBLIC_BACKEND_BASE_URL } from '$env/static/public';

import { superValidate } from "sveltekit-superforms";
import { formSchema, modifyFormSchema } from "./schema";
import { zod } from "sveltekit-superforms/adapters";

export const load: PageLoad = async ({ fetch }) => {
  // Fetch Data
  const res = await fetch(`${PUBLIC_BACKEND_BASE_URL}/feeds/`);
  if (!res.ok) throw new Error('Bad response');
  const feeds: Feed[] = await res.json();
  return {
    feeds,
    form: await superValidate(zod(formSchema)), // Schema for Form (cached)
    modifyForm: await superValidate(zod(modifyFormSchema))
  };
  // If an error is thrown during load, the nearest +error.svelte will be rendered.
  // For expected errors, use the error helper from @sveltejs/kit to specify
  // the HTTP status code and an optional message:
  // import { error } from '@sveltejs/kit';
  // error(404, 'Not found');
};