<script lang="ts">
  import '../app.css';
  import logo from '$lib/media/logo.svg';
  import galaxy from '$lib/media/galaxy.jpg';

  let { children } = $props();
</script>

<nav class="bg-white dark:bg-gray-900 fixed w-full z-20 top-0 start-0 border-b border-gray-200 dark:border-gray-600">
    <div class="max-w-screen-xl flex flex-wrap items-center justify-between mx-auto p-4">
        <a href="/" class="flex items-center space-x-3 rtl:space-x-reverse">
            <img src="{logo}" class="h-8" alt="RSS Notifier Logo"/>
            <span class="self-center text-2xl font-semibold whitespace-nowrap dark:text-white">RSS Notifier</span>
        </a>
    </div>
</nav>

<div class="w-full -z-20 top-0 start-0 bg-fixed bg-center bg-cover min-h-screen backdrop-blur-sm"
     style="background-image: url({galaxy})">
<!-- Overlay for darkening and adding gradient -->
<div class="absolute -z-10 inset-0 bg-gradient-to-b from-transparent to-gray-500/50"></div>
<!-- Logo h-8 plus the div p-4 = 4+8+4=16 of total navbar height if there are no other influences-->
<div class="mt-16 flex flex-col">
    {@render children()}
</div>
</div>