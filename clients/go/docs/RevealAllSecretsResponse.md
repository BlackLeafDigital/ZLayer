# RevealAllSecretsResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Environment** | **string** | The environment id the secrets were revealed from. | 
**Secrets** | **map[string]string** | Name → plaintext value map. Includes every secret in the scope. | 

## Methods

### NewRevealAllSecretsResponse

`func NewRevealAllSecretsResponse(environment string, secrets map[string]string, ) *RevealAllSecretsResponse`

NewRevealAllSecretsResponse instantiates a new RevealAllSecretsResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewRevealAllSecretsResponseWithDefaults

`func NewRevealAllSecretsResponseWithDefaults() *RevealAllSecretsResponse`

NewRevealAllSecretsResponseWithDefaults instantiates a new RevealAllSecretsResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetEnvironment

`func (o *RevealAllSecretsResponse) GetEnvironment() string`

GetEnvironment returns the Environment field if non-nil, zero value otherwise.

### GetEnvironmentOk

`func (o *RevealAllSecretsResponse) GetEnvironmentOk() (*string, bool)`

GetEnvironmentOk returns a tuple with the Environment field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetEnvironment

`func (o *RevealAllSecretsResponse) SetEnvironment(v string)`

SetEnvironment sets Environment field to given value.


### GetSecrets

`func (o *RevealAllSecretsResponse) GetSecrets() map[string]string`

GetSecrets returns the Secrets field if non-nil, zero value otherwise.

### GetSecretsOk

`func (o *RevealAllSecretsResponse) GetSecretsOk() (*map[string]string, bool)`

GetSecretsOk returns a tuple with the Secrets field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSecrets

`func (o *RevealAllSecretsResponse) SetSecrets(v map[string]string)`

SetSecrets sets Secrets field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


