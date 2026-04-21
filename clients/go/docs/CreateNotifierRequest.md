# CreateNotifierRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Config** | [**NotifierConfig**](NotifierConfig.md) | Channel-specific configuration. | 
**Kind** | [**NotifierKind**](NotifierKind.md) | Notification channel type. | 
**Name** | **string** | Notifier name. | 

## Methods

### NewCreateNotifierRequest

`func NewCreateNotifierRequest(config NotifierConfig, kind NotifierKind, name string, ) *CreateNotifierRequest`

NewCreateNotifierRequest instantiates a new CreateNotifierRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCreateNotifierRequestWithDefaults

`func NewCreateNotifierRequestWithDefaults() *CreateNotifierRequest`

NewCreateNotifierRequestWithDefaults instantiates a new CreateNotifierRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetConfig

`func (o *CreateNotifierRequest) GetConfig() NotifierConfig`

GetConfig returns the Config field if non-nil, zero value otherwise.

### GetConfigOk

`func (o *CreateNotifierRequest) GetConfigOk() (*NotifierConfig, bool)`

GetConfigOk returns a tuple with the Config field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetConfig

`func (o *CreateNotifierRequest) SetConfig(v NotifierConfig)`

SetConfig sets Config field to given value.


### GetKind

`func (o *CreateNotifierRequest) GetKind() NotifierKind`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *CreateNotifierRequest) GetKindOk() (*NotifierKind, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *CreateNotifierRequest) SetKind(v NotifierKind)`

SetKind sets Kind field to given value.


### GetName

`func (o *CreateNotifierRequest) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *CreateNotifierRequest) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *CreateNotifierRequest) SetName(v string)`

SetName sets Name field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


