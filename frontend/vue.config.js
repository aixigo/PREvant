const webpack = require( 'webpack' );

module.exports = {
   chainWebpack: (config) => {
      config.resolve.alias.set('vue', '@vue/compat')

      config.module
         .rule('vue')
         .use('vue-loader')
         .tap((options) => {
            return {
               ...options,
               compilerOptions: {
                  compatConfig: {
                     MODE: 2
                  }
               }
            }
         })
   },

   configureWebpack: {
      entry:['@babel/polyfill','./src/main.js'],
      plugins: [
         new webpack.ProvidePlugin({
            'jQuery': 'jquery',
            '$': 'jquery',
            Popper: ['popper.js', 'default']
         })
      ]
   },
   transpileDependencies: ["bootstrap-material-design"],
   devServer: {
      port: 9001,
      proxy: {
         '/api': {
            target: 'http://127.0.0.1:8000'
         },
         '/openapi.yaml': {
            target: 'http://127.0.0.1:8000'
         }
      }
   }
}

