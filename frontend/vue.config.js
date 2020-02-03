const webpack = require( 'webpack' );

module.exports = {
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
            target: 'http://localhost:8000'
         },
         '/openapi.yaml': {
            target: 'http://localhost:8000'
         }
      }
   }
}