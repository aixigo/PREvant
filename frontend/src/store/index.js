import Vue from 'vue';
import Vuex from 'vuex'

Vue.use( Vuex );

export default new Vuex.Store( {
   state: {
      apps: {},
      tickets: {},
      status: {},
      appNameFilter: ''
   },
   getters: {

      rootUrl: state => state.status.rootUrl,
      swaggerUiAvailable: state => state.status.swaggerUiAvailable,
      portainerAvailable: state => state.status.portainerAvailable,

      swaggerUrl: state => {
         if ( state.status.swaggerUiAvailable ) {
            return `${state.status.rootUrl}api/swagger.yaml`;
         }

         return '';
      },

      portainerUrl: state => {
         if ( state.status.portainerAvailable ) {
            return `${state.status.rootUrl}portainer/`;
         }

         return '';
      },

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

         return apps.filter( app => !state.appNameFilter || app.name.toLocaleLowerCase().indexOf( state.appNameFilter ) >= 0 );

         function appDetails( name ) {
            const appContainers = state.apps[ name ];
            const ticket = state.tickets[ name ];

            const containers = [
               ...appContainers
                  .map( ( { vhost, url, version, containerType, containerId } ) => {
                     return {
                        vhost, url, version, label: name, containerType, containerId
                     };
                  } )
            ].map( container => {
               let swaggerUrl = undefined;
               let logsUrl = undefined;
               if ( state.status.swaggerUiAvailable && container.version && container.version.api ) {
                  swaggerUrl = container.url.replace( `/${container.vhost}/`, '/swagger-ui/' )
                     + `?url=${container.version.api.url}`;
               }
               if ( state.status.portainerAvailable && container.containerId ) {
                  logsUrl = `${state.status.rootUrl}/portainer/#/containers/${container.containerId}/logs`;
               }
               return Object.assign( {}, container, { swaggerUrl, logsUrl } );
            } );
            containers.sort( byVhost );
            return { name, ticket, containers };
         }

         function byVhost( containerA, containerB ) {
            const [ keyA, keyB ] = [ containerA, containerB ]
               .map( ( { vhost } ) => `${vhost.endsWith( '-frontend' ) ? 0 : 1 }-${vhost}` );
            return keyA < keyB ? -1 : 1;
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

      storeStatus( state, status ) {
         state.status = status;
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
            fetch( '/api/' )
               .then( response => response.json() ),
            fetch( '/api/apps' )
               .then( response => response.json() ),
            fetch( '/api/apps/tickets' )
               .then( response => {
                  if (response.ok) {
                     return response.json()
                  }
                  return {};
               } )
         ]).then((values) => {
            context.commit( "storeStatus", values[0] );
            context.commit( "storeTickets", values[2] );
            context.commit( "storeApps", values[1] );
            context.dispatch( 'fetchVersions' );
         });
      },

      fetchVersions( context ) {
         for ( let name of Object.keys( context.state.apps ) ) {

            let promises = [];
            context.state.apps[ name ].forEach( ( container, containerIndex ) => {
               if ( container.vhost.endsWith( '-frontend' ) ) {
                  return;
               }

               promises.push( fetch( container.versionUrl )
                  .then( res => res.ok ? res.json() : { 'build.time': 'N/A', 'git.revision': 'N/A' } )
                  .then( version => ( { name, containerIndex, version  } ) ) );
            } );

            Promise.all(promises)
               .then( versions => context.commit( "storeVersion", versions ) );
         }
      }
   }
} );

