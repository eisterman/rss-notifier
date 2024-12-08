<script lang="ts">
  import * as _ from "lodash-es";
  import type { Feed } from '$types';  // My Types
  import * as Table from "$lib/components/ui/table/index.js";
  import * as ContextMenu from "$lib/components/ui/context-menu/index.js";
  import * as Dialog from "$lib/components/ui/dialog/index.js";
  import * as Form from "$lib/components/ui/form/index.js";
  import { Input } from "$lib/components/ui/input/index.js";
  import { Button } from "$lib/components/ui/button/index.js";
  import { Delete, Plus, SquarePen } from 'lucide-svelte';
  import { PUBLIC_BACKEND_BASE_URL } from '$env/static/public';

  import { setMessage, superForm, setError } from "sveltekit-superforms";
  // import SuperDebug from "sveltekit-superforms";
  import { formSchema, modifyFormSchema } from "./schema";
  import { zod } from "sveltekit-superforms/adapters";
  import { invalidate } from "$app/navigation";

  let { data } = $props();

  // To get things better with Update, I can move the Update to a new path that load the data in his +page.ts
  // I need to make it so that under it there is still renderized this root path, not easy.
  // Can be needed all the component moved in the layout? Mmh...

  const form = superForm(
    data.form,
    {
      SPA: true,
      validators: zod(formSchema),
      async onUpdate({ form }) {
        // For SPA the first process of request to the server for validation is always ignored
        // and here it's the best place where to put the AJAX
        // Form validation
        if (form.valid) {
          // Call an external API with form.data, await the result and update form
          const res = await fetch(`${PUBLIC_BACKEND_BASE_URL}/feeds/`, {
            method: 'POST',
            headers: {
              'Content-Type': 'application/json',
            },
            body: JSON.stringify(form.data)
          });
          if (!res.ok) setError(form, 'Error during POST');
          setMessage(form, 'Valid data!');
          createIsOpen = false;
          await invalidate(`${PUBLIC_BACKEND_BASE_URL}/feeds/`);
        }
      }
    }
  );
  const { form: formData, enhance } = form;

  const modifyForm = superForm(
    data.modifyForm,
    {
      SPA: true,
      validators: zod(modifyFormSchema),
      async onUpdate({ form }) {
        // For SPA the first process of request to the server for validation is always ignored
        // and here it's the best place where to put the AJAX
        // Form validation
        if (form.valid) {
          // Call an external API with form.data, await the result and update form
          const res = await fetch(`${PUBLIC_BACKEND_BASE_URL}/feeds/${form.data.id}/`, {
            method: 'PUT',
            headers: {
              'Content-Type': 'application/json',
            },
            body: JSON.stringify(_.omit(form.data, ['id']))
          });
          if (!res.ok) setError(form, 'Error during PUT');
          setMessage(form, 'Valid data!');
          modifyIsOpen = false;
          await invalidate(`${PUBLIC_BACKEND_BASE_URL}/feeds/${form.data.id}/`);
        }
      }
    }
  );
  const { form: modifyFormData, enhance: modifyEnhance } = modifyForm;

  let createIsOpen = $state(false);

  function create_form() {
    createIsOpen = !createIsOpen;
  }

  let modifyIsOpen = $state(false);

  function modify_form(feed: Feed) {
    // This Store set causes a warning, that probably is a false positive.
    // Seems in bits-ui there are some variables that are not properly $bindable
    // making the page in Dev mode raise a warning, even if everything is properly working.
    // We can ignore this for now, hoping it will solve itself in the next months.
    // ref: https://github.com/sveltejs/svelte/issues/13607
    modifyFormData.set(feed);
    modifyIsOpen = !modifyIsOpen;
  }

  async function delete_feed(id: number) {
    const res = await fetch(`${PUBLIC_BACKEND_BASE_URL}/feeds/${id}/`, {
      method: 'DELETE',
    });
    if (!res.ok) throw new Error('Bad response');
    await invalidate(`${PUBLIC_BACKEND_BASE_URL}/feeds/`);
  }
</script>

<div class="bg-white rounded-3xl m-2">
    <div class="p-4">
        <div class="flex justify-end">
            <Dialog.Root open={createIsOpen} onOpenChange={create_form}>
                <Dialog.Trigger>
                    <Button>
                        <Plus/>
                        Create
                    </Button>
                </Dialog.Trigger>
                <Dialog.Content>
                    <Dialog.Header>
                        <Dialog.Title>Create new RSS Feed subscription</Dialog.Title>
                    </Dialog.Header>
                    <form method="POST" use:enhance>
                        <Form.Field form={form} name="name">
                            <Form.Control>
                                {#snippet children({ props })}
                                    <Form.Label>Name</Form.Label>
                                    <Input {...props} bind:value={$formData.name}/>
                                {/snippet}
                            </Form.Control>
                            <Form.Description>Name of the RS Feed.</Form.Description>
                            <Form.FieldErrors/>
                        </Form.Field>
                        <Form.Field form={form} name="feed_url">
                            <Form.Control>
                                {#snippet children({ props })}
                                    <Form.Label>Feed URL</Form.Label>
                                    <Input {...props} bind:value={$formData.feed_url}/>
                                {/snippet}
                            </Form.Control>
                            <Form.Description>URL to the XML of the RSS Feed.</Form.Description>
                            <Form.FieldErrors/>
                        </Form.Field>
                        <Form.Button>Submit</Form.Button>
                    </form>
                </Dialog.Content>
            </Dialog.Root>
        </div>
        <div>
            <Table.Root>
                <Table.Header>
                    <Table.Row>
                        <Table.Head>Feed Name</Table.Head>
                        <Table.Head>Feed URL</Table.Head>
                        <Table.Head>Last Mail Sent</Table.Head>
                        <Table.Head class="text-right">Actions</Table.Head>
                    </Table.Row>
                </Table.Header>
                <Table.Body>
                    <Dialog.Root open={modifyIsOpen} onOpenChange={(o) => (modifyIsOpen = o)}>
                        <Dialog.Content>
                            <Dialog.Header>
                                <Dialog.Title>Modify RSS Feed subscription</Dialog.Title>
                            </Dialog.Header>
                            <form method="POST" use:modifyEnhance>
                                <Form.Field form={modifyForm} name="name">
                                    <Form.Control>
                                        {#snippet children({ props })}
                                            <Form.Label>Name</Form.Label>
                                            <Input {...props}
                                                   bind:value={$modifyFormData.name}/>
                                        {/snippet}
                                    </Form.Control>
                                    <Form.Description>Name of the RS Feed.</Form.Description>
                                    <Form.FieldErrors/>
                                </Form.Field>
                                <Form.Field form={modifyForm} name="feed_url">
                                    <Form.Control>
                                        {#snippet children({ props })}
                                            <Form.Label>Feed URL</Form.Label>
                                            <Input {...props}
                                                   bind:value={$modifyFormData.feed_url}/>
                                        {/snippet}
                                    </Form.Control>
                                    <Form.Description>URL to the XML of the RSS Feed.
                                    </Form.Description>
                                    <Form.FieldErrors/>
                                </Form.Field>
                                <Form.Button>Submit</Form.Button>
                            </form>
                        </Dialog.Content>
                    </Dialog.Root>
                    {#each data.feeds as feed (feed.id)}
                        <ContextMenu.Root>
                            <ContextMenu.Trigger>
                                {#snippet child({ props })}
                                    <Table.Row {...props}>
                                        <Table.Cell class="font-medium">{feed.name}</Table.Cell>
                                        <Table.Cell>{feed.feed_url}</Table.Cell>
                                        <Table.Cell>{feed.last_pub_date}</Table.Cell>
                                        <Table.Cell class="text-right">
                                            <Button onclick={() => modify_form(feed)}>
                                                <SquarePen/>
                                                Modify
                                            </Button>
                                            <Dialog.Root>
                                                <Dialog.Trigger>
                                                    <Button variant="destructive">
                                                        <Delete/>
                                                        Delete
                                                    </Button>
                                                </Dialog.Trigger>
                                                <Dialog.Content>
                                                    <Dialog.Header>
                                                        <Dialog.Title>Are you sure absolutely sure?</Dialog.Title>
                                                        <Dialog.Description>
                                                            This action cannot be undone. This will permanently delete
                                                            the feed.
                                                        </Dialog.Description>
                                                    </Dialog.Header>
                                                    <div class="flex justify-end">
                                                        <Dialog.Close>
                                                            <Button class="mx-1" variant="outline">No Please!</Button>
                                                            <Button class="mx-1" variant="destructive"
                                                                    onclick={async () => await delete_feed(feed.id)}>
                                                                Confirm
                                                            </Button>
                                                        </Dialog.Close>
                                                    </div>
                                                </Dialog.Content>
                                            </Dialog.Root>
                                        </Table.Cell>
                                    </Table.Row>
                                {/snippet}
                            </ContextMenu.Trigger>
                            <ContextMenu.Content>
                                <ContextMenu.Item>
                                    <SquarePen size="20" class="mx-2"/>
                                    Modify
                                </ContextMenu.Item>
                                <ContextMenu.Item>
                                    <Delete size="20" class="mx-2"/>
                                    Delete
                                </ContextMenu.Item>
                            </ContextMenu.Content>
                        </ContextMenu.Root>
                    {/each}
                </Table.Body>
            </Table.Root>
        </div>
    </div>
</div>