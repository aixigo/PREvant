package com.aixigo.preview.servant.rest;

/*-
 * ========================LICENSE_START=================================
 * PREvant REST API Integration Tests
 * %%
 * Copyright (C) 2018 aixigo AG
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

import com.aixigo.preview.servant.rest.junit.extension.PREvantRestApiExtension;
import com.aixigo.preview.servant.rest.model.ServiceConfiguration;
import io.restassured.response.ExtractableResponse;
import io.restassured.response.Response;
import io.restassured.response.ResponseBodyExtractionOptions;
import io.restassured.response.ValidatableResponse;
import org.junit.Rule;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.extension.ExtendWith;
import org.junit.jupiter.migrationsupport.rules.EnableRuleMigrationSupport;
import org.junit.rules.TemporaryFolder;

import java.io.BufferedWriter;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.OutputStreamWriter;
import java.net.URI;
import java.util.List;

import static io.restassured.RestAssured.given;
import static java.util.Arrays.asList;
import static java.util.Collections.singletonList;

@EnableRuleMigrationSupport
@ExtendWith(PREvantRestApiExtension.class)
class PREvantRestApiDockerTest {

    private static final int WAIT_FOR_SERVICES = 5_000;

    /**
     * TODO: Replace {@code TempDirectory} extension when junit 5.4.0 has been released
     */
    @Rule
    public final TemporaryFolder temporaryFolder = new TemporaryFolder();

    @Test
    void shouldDeployDockerContainer_WhenRequestToDeployService(URI restApiURI) throws Exception {
        String uri = postServiceConfiguration(restApiURI, "master", "nginx", "library", "nginx")
                .extract()
                .body()
                .path("[0].url");

        Thread.sleep(WAIT_FOR_SERVICES);
        given().baseUri(uri)
                .get()
                .then()
                .statusCode(200);

        deleteApp(restApiURI, "master")
                .statusCode(200);
    }

    @Test
    void shouldLinkDockerContainerToNetwork_WhenRequestToDeployService(URI restApiURI) throws Exception {
        postServiceConfiguration(restApiURI, "master", "httpd", "library", "httpd");
        postServiceConfiguration(restApiURI, "master",
                new ServiceConfiguration("nginx", "library", "nginx")
                        .addVolume(createNginxConfigVolume()));

        postServiceConfiguration(restApiURI, "master", "httpd", "library", "httpd");

        Thread.sleep(WAIT_FOR_SERVICES);
        given().baseUri(restApiURI.toString())
                .get("/master/nginx/")
                .then()
                .statusCode(200);

        deleteApp(restApiURI, "master")
                .statusCode(200);
    }

    @Test
    void shouldReplicateRemainingServicesFromMasterWhenDeployingToFeatureBranch(URI restApiURI) throws Exception {
        postServiceConfiguration(restApiURI, "master", asList(
                new ServiceConfiguration("httpd", "library", "httpd"),
                new ServiceConfiguration("nginx", "library", "nginx")
        ));

        postServiceConfiguration(restApiURI, "master-1x", "httpd", "library", "httpd")
                .statusCode(200);

        Thread.sleep(WAIT_FOR_SERVICES);
        given().baseUri(restApiURI.toString())
                .get("/master-1x/nginx/")
                .then()
                .statusCode(200);

        given().baseUri(restApiURI.toString())
                .get("/master-1x/httpd/")
                .then()
                .statusCode(200);

        deleteApp(restApiURI, "master-1x")
                .statusCode(200);
        deleteApp(restApiURI, "master")
                .statusCode(200);
    }

    private String createNginxConfigVolume() throws IOException {
        File nginxConfig = temporaryFolder.newFile();
        try (BufferedWriter bw = new BufferedWriter(new OutputStreamWriter(new FileOutputStream(nginxConfig)))) {
            bw.write("server {");
            bw.newLine();
            bw.write("  resolver 127.0.0.11 valid=0s;");
            bw.newLine();
            bw.write("  listen       80;");
            bw.newLine();
            bw.write("  server_name  localhost;");
            bw.newLine();
            bw.write("  access_log   /var/log/nginx/access.log main;");
            bw.newLine();
            bw.write("  location / {");
            bw.newLine();
            bw.write("    set $upstream httpd:80;");
            bw.newLine();
            bw.write("    proxy_pass http://$upstream;");
            bw.newLine();
            bw.write("  }");
            bw.newLine();
            bw.write("}");
            bw.newLine();
        }

        return nginxConfig.getAbsolutePath() + ":/etc/nginx/conf.d/default.conf";
    }

    private ValidatableResponse postServiceConfiguration(URI restApiURI, String appName, String serviceName, String imageUser, String imageRepository) {
        return postServiceConfiguration(restApiURI, appName, new ServiceConfiguration(serviceName, imageUser, imageRepository));
    }

    private ValidatableResponse postServiceConfiguration(URI restApiURI, String appName, ServiceConfiguration serviceConfiguration) {
        return postServiceConfiguration(restApiURI, appName, singletonList(serviceConfiguration));
    }

    private ValidatableResponse postServiceConfiguration(URI restApiURI, String appName, List<ServiceConfiguration> serviceConfigurations) {
        return given()
                .contentType("application/json")
                .baseUri(restApiURI.toString())
                .body(serviceConfigurations)
                .when()
                .post("api/apps/" + appName)
                .then();
    }

    private ValidatableResponse deleteApp(URI restApiURI, String appName) {
        return given()
                .contentType("application/json")
                .baseUri(restApiURI.toString())
                .delete("api/apps/" + appName)
                .then();
    }

}
