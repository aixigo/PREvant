module.exports = {
   plugins: [
      require( 'autoprefixer' )( {
         grid: true,
         browsers: [ 'ie >= 11', 'Firefox >= 60', 'Firefox ESR', 'Chrome >= 60' ]
      } )
   ]
};