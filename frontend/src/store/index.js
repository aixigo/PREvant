/*-
 * ========================LICENSE_START=================================
 * PREvant Frontend
 * %%
 * Copyright (C) 2018 - 2019 aixigo AG
 * %%
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 * 
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 * 
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
 * THE SOFTWARE.
 * =========================LICENSE_END==================================
 */

import Vue from 'vue';
import Vuex from 'vuex'

Vue.use( Vuex );

const SERVICE_TYPE_ORDER = [
    'instance',
    'replica',
    'app-companion',
    'service-companion'
];

function timeout(ms, promise) {
    return new Promise(function(resolve, reject) {
        setTimeout(function() {
            reject(new Error("timeout"))
        }, ms)
        promise.then(resolve, reject)
    })
}

export default new Vuex.Store( {
   state: {
      apps: {},
      tickets: {},
      appNameFilter: ''
   },
   getters: {

      reviewApps: state => {
         if ( state.apps === undefined || Object.keys( state.apps ).length == 0 ) {
            return [];
         }

         const apps = [
            appDetails( 'master' ),
            ...Object.keys( state.apps )
               .filter( _ => _ != 'master' )
               .map( appDetails )
               .sort( byAppNameDesc )
         ];

         return apps
             .filter( app => app.name != null )
             .filter( app => !state.appNameFilter || app.name.toLocaleLowerCase().indexOf( state.appNameFilter ) >= 0 );

         function appDetails( name ) {
            const appContainers = state.apps[ name ];

            if (appContainers == null) {
               return {};
            }

            const ticket = state.tickets[ name ];

            const containers = [
               ...appContainers
                  .map( ( { name, url, version, type } ) => {
                     return {
                         name, url, version, type
                     };
                  } )
            ].map( container => {
               let apiUrl = undefined;
               if ( container.version && container.version.api ) {
                  apiUrl = container.version.api.url;
               }

               return Object.assign( {}, container, { apiUrl } );
            } );
            containers.sort( byTypeAndName );
            return { name, ticket, containers };
         }

          function byTypeAndName(containerA, containerB) {
              const typeIndexA = SERVICE_TYPE_ORDER.indexOf(containerA.type);
              const typeIndexB = SERVICE_TYPE_ORDER.indexOf(containerB.type);

              if (typeIndexA !== typeIndexB) {
                  return typeIndexA < typeIndexB ? -1 : 1;
              }

              return containerA.name < containerB.name ? -1 : 1;
          }

         function byAppNameDesc( appA, appB ) {
            const [ keyA, keyB ] = [ appA, appB ].map( ( { name } ) => name );
            return keyA > keyB ? -1 : 1;
         }
      }
   },
   mutations: {
      storeApps( state, apps ) {
         state.apps = apps;
      },

      storeTickets( state, tickets ) {
         state.tickets = tickets;
      },

      storeVersion( state, e ) {
         e.forEach( ({ name, containerIndex, version }) => {
            Vue.set( state.apps[ name ][ containerIndex ], 'version', version );
         } );
      },

      filterByAppName( state, appNameFilter ) {
         state.appNameFilter = appNameFilter.toLocaleLowerCase();
      }
   },
   actions: {
      fetchData( context ) {
         Promise.all([
            fetch( '/api/apps' )
               .then( response => {
                   if (response.ok && response.status === 200) {
                       return response.json();
                   }
                   return Promise.resolve(() => {});
               } ),
            fetch( '/api/apps/tickets' )
               .then( response => {
                  if (response.ok && response.status === 200) {
                     return response.json();
                  }
                   return Promise.resolve(() => {});
               } )
         ]).then((values) => {
            context.commit( "storeTickets", values[1] );
            context.commit( "storeApps", values[0] );
            context.dispatch( 'fetchVersions' );
         });
      },

      fetchVersions( context ) {
         for ( let name of Object.keys( context.state.apps ) ) {

            let promises = [];
            context.state.apps[ name ].forEach( ( container, containerIndex ) => {
               const undefinedVersion = { 'build.time': 'N/A', 'git.revision': 'N/A' };
               if ( container.versionUrl == null ) {
                  promises.push( Promise.resolve( ( { name, containerIndex, version: undefinedVersion } ) ) );
                  return;
               }

               promises.push( timeout( 2000, fetch( container.versionUrl )
                  .then( res => res.ok ? res.json() : undefinedVersion )
                  .then( version => ( { name, containerIndex, version } ) ) )
                  .catch( err => ( { name, containerIndex, version: undefinedVersion } ) ) );
            } );

            Promise.all(promises)
               .then( versions => context.commit( "storeVersion", versions ) );
         }
      }
   }
} );

