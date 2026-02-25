<template>
   <Navbar/>
   <main>
      <RouterView />
   </main>
   <Footer/>
</template>

<script setup>
import Footer from './components/Footer.vue'
import Navbar from './components/Navbar.vue'
</script>
